//! The Core ML backend: every Objective-C call of the crate, so all its `unsafe`.
//!
//! Rules (README.md): every call runs inside an autorelease pool, so a worker thread that lives for the whole process
//! leaks nothing. Objective-C exceptions are **not caught** (that needs `objc2`'s `exception` feature, which compiles
//! an Objective-C helper with `cc`; LightCraft is pure Rust): Core ML is never given a reason to throw. Every class
//! method and selector used is checked with `respondsToSelector:` in [`available`]; every object a call may return as
//! nil is received as an `Option` and checked; errors come back through an `NSError` out-parameter; names are checked
//! against the model's description and shapes against the data before the call. The Rust closures handed to Core ML
//! (the byte handlers) only copy numbers and cannot panic.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::StackBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, ClassType, msg_send, sel};
use objc2_core_ml::{
    MLComputeUnits, MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel, MLModelConfiguration, MLModelDescription, MLMultiArray,
    MLMultiArrayDataType,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSInteger, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL};

use crate::layout::{Elem, contiguous, gather};
use crate::{ComputeUnits, Error, Output, Result};

/// A loaded `MLModel`.
pub(crate) struct Handle(Retained<MLModel>);

// SAFETY: an `MLModel` has no thread affinity (Apple documents loading and predicting off the main thread), and its
// reference count is atomic. `Model` keeps the handle behind a `Mutex`, so only one thread uses it at a time.
unsafe impl Send for Handle {}

/// (class, selector) pairs called on instances; checked once.
fn instance_selectors() -> [(&'static std::ffi::CStr, Sel); 9] {
    [
        (c"MLModel", sel!(predictionFromFeatures:error:)),
        (c"MLModel", sel!(modelDescription)),
        (c"MLModelDescription", sel!(inputDescriptionsByName)),
        (c"MLModelDescription", sel!(outputDescriptionsByName)),
        (c"MLModelConfiguration", sel!(setComputeUnits:)),
        (c"MLMultiArray", sel!(initWithShape:dataType:error:)),
        (c"MLMultiArray", sel!(getBytesWithHandler:)),
        (c"MLMultiArray", sel!(getMutableBytesWithHandler:)),
        (c"MLDictionaryFeatureProvider", sel!(initWithDictionary:error:)),
    ]
}

/// macOS 13 or later (`MLComputeUnitsCPUAndNeuralEngine`, the byte handlers) with every class and selector used.
pub(crate) fn available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        autoreleasepool(|_| {
            let at_least_13 = NSOperatingSystemVersion { majorVersion: 13, minorVersion: 0, patchVersion: 0 };
            NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(at_least_13)
                && instance_selectors().iter().all(|(class, s)| AnyClass::get(class).is_some_and(|c| c.responds_to(*s)))
                && AnyClass::get(c"MLModel").is_some_and(|c| {
                    class_responds(c, sel!(compileModelAtURL:error:)) && class_responds(c, sel!(modelWithContentsOfURL:configuration:error:))
                })
                && AnyClass::get(c"MLFeatureValue").is_some_and(|c| class_responds(c, sel!(featureValueWithMultiArray:)))
        })
    })
}

fn class_responds(class: &AnyClass, sel: Sel) -> bool {
    // SAFETY: `respondsToSelector:` is NSObject's, answered by every class object; it takes a selector and returns a
    // BOOL.
    unsafe { msg_send![class, respondsToSelector: sel] }
}

fn url_of(path: &Path) -> Result<Retained<NSURL>> {
    let Some(s) = path.to_str() else { return Err(Error::Io(format!("{} is not a UTF-8 path", path.display()))) };
    Ok(NSURL::fileURLWithPath(&NSString::from_str(s)))
}

fn describe(err: Option<Retained<NSError>>) -> String {
    err.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "no reason given".into())
}

pub(crate) fn compile(package: &Path) -> Result<PathBuf> {
    if !available() {
        return Err(Error::Unavailable);
    }
    autoreleasepool(|_| {
        let url = url_of(package)?;
        let mut err: Option<Retained<NSError>> = None;
        // SAFETY: a class method checked in `available`, sent with a non-nil file URL and an NSError out-parameter; the
        // result is received as optional (nil on failure).
        let out: Option<Retained<NSURL>> = unsafe { msg_send![MLModel::class(), compileModelAtURL: &*url, error: &mut err] };
        let out = out.ok_or_else(|| Error::Compile(describe(err)))?;
        out.path().map(|p| PathBuf::from(p.to_string())).ok_or_else(|| Error::Compile("Core ML gave no folder".into()))
    })
}

fn units_of(units: ComputeUnits) -> MLComputeUnits {
    match units {
        ComputeUnits::All => MLComputeUnits::All,
        ComputeUnits::CpuAndNeuralEngine => MLComputeUnits::CPUAndNeuralEngine,
        ComputeUnits::CpuAndGpu => MLComputeUnits::CPUAndGPU,
        ComputeUnits::CpuOnly => MLComputeUnits::CPUOnly,
    }
}

/// The keys of a feature-description dictionary, sorted.
fn names(dict: Option<Retained<NSDictionary<NSString, AnyObject>>>) -> Vec<String> {
    let mut out: Vec<String> = dict.map(|d| d.allKeys().iter().map(|k| k.to_string()).collect()).unwrap_or_default();
    out.sort();
    out
}

pub(crate) fn load(compiled: &Path, units: ComputeUnits) -> Result<(Handle, Vec<String>, Vec<String>)> {
    if !available() {
        return Err(Error::Unavailable);
    }
    autoreleasepool(|_| {
        let url = url_of(compiled)?;
        // SAFETY: `new` on NSObject's subclass `MLModelConfiguration` (default configuration); `setComputeUnits:` is
        // checked in `available` and takes one of the four documented values.
        let config = unsafe {
            let c = MLModelConfiguration::new();
            c.setComputeUnits(units_of(units));
            c
        };
        let mut err: Option<Retained<NSError>> = None;
        // SAFETY: a class method checked in `available`, with a non-nil URL and configuration and an NSError
        // out-parameter; the result is received as optional.
        let model: Option<Retained<MLModel>> =
            unsafe { msg_send![MLModel::class(), modelWithContentsOfURL: &*url, configuration: &*config, error: &mut err] };
        let model = model.ok_or_else(|| Error::Load(describe(err)))?;
        // SAFETY: getters checked in `available`, received as optional; the dictionaries map names to
        // `MLFeatureDescription`s, read here only as objects for their keys.
        let (inputs, outputs) = unsafe {
            let desc: Option<Retained<MLModelDescription>> = msg_send![&*model, modelDescription];
            let desc = desc.ok_or_else(|| Error::Load("the model has no description".into()))?;
            let i: Option<Retained<NSDictionary<NSString, AnyObject>>> = msg_send![&*desc, inputDescriptionsByName];
            let o: Option<Retained<NSDictionary<NSString, AnyObject>>> = msg_send![&*desc, outputDescriptionsByName];
            (names(i), names(o))
        };
        if inputs.is_empty() || outputs.is_empty() {
            return Err(Error::Load("the model lists no inputs or no outputs".into()));
        }
        Ok((Handle(model), inputs, outputs))
    })
}

fn numbers(values: &[usize]) -> Result<Retained<NSArray<NSNumber>>> {
    let n: Vec<Retained<NSNumber>> = values
        .iter()
        .map(|&v| NSInteger::try_from(v).map(NSNumber::numberWithInteger).map_err(|_| Error::Input(format!("{v} is too large"))))
        .collect::<Result<_>>()?;
    Ok(NSArray::from_retained_slice(&n))
}

fn usizes(values: &NSArray<NSNumber>) -> Result<Vec<usize>> {
    values.iter().map(|n| usize::try_from(n.integerValue()).map_err(|_| Error::Output(format!("{} is not a size", n.integerValue())))).collect()
}

/// A Float32 `MLMultiArray` of `shape` holding `data` (checked by the caller: `shape`'s product is `data.len()`).
fn multi_array(shape: &[usize], data: &[f32]) -> Result<Retained<MLMultiArray>> {
    let dims = numbers(shape)?;
    let mut err: Option<Retained<NSError>> = None;
    // SAFETY: `initWithShape:dataType:error:` (checked in `available`) on a fresh allocation, with a non-empty shape of
    // positive sizes and a documented data type; received as optional, with an NSError out-parameter.
    let array: Option<Retained<MLMultiArray>> =
        unsafe { msg_send![MLMultiArray::alloc(), initWithShape: &*dims, dataType: MLMultiArrayDataType::Float32, error: &mut err] };
    let array = array.ok_or_else(|| Error::Input(describe(err)))?;
    let want = contiguous(shape);
    let written: RefCell<Result<()>> = RefCell::new(Err(Error::Input("Core ML gave no buffer".into())));
    let handler = StackBlock::new(|ptr: NonNull<c_void>, size: NSInteger, strides: NonNull<NSArray<NSNumber>>| {
        // SAFETY: Core ML passes its strides array, valid for the handler's duration.
        let strides = usizes(unsafe { strides.as_ref() });
        let fits = usize::try_from(size).is_ok_and(|s| s == std::mem::size_of_val(data));
        *written.borrow_mut() = match strides {
            Ok(s) if s == want && fits => {
                let bytes = std::mem::size_of_val(data);
                // SAFETY: Core ML's buffer is `size` bytes, writable for the handler's duration and Float32 (asked for
                // above); `size` equals `data`'s byte length, so the copy stays inside it. Copied as bytes, so the
                // buffer's alignment does not matter.
                unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), ptr.as_ptr().cast::<u8>(), bytes) };
                Ok(())
            }
            Ok(s) => Err(Error::Input(format!("Core ML laid the input out as {s:?} in {size} bytes"))),
            Err(e) => Err(e),
        };
    });
    // SAFETY: checked in `available`; the handler runs synchronously, before this call returns.
    unsafe { array.getMutableBytesWithHandler(&handler) };
    written.into_inner().map(|()| array)
}

fn elem_of(t: MLMultiArrayDataType) -> Result<Elem> {
    match t {
        MLMultiArrayDataType::Float16 => Ok(Elem::F16),
        MLMultiArrayDataType::Float32 => Ok(Elem::F32),
        MLMultiArrayDataType::Float64 => Ok(Elem::F64),
        other => Err(Error::Output(format!("the output holds data type {:#x}, not floats", other.0))),
    }
}

/// The numbers of `array`, row-major.
fn read(array: &MLMultiArray) -> Result<Output> {
    // SAFETY: plain getters of a live `MLMultiArray`, returning a value type and two non-nil arrays.
    let (elem, shape, strides) = unsafe { (elem_of(array.dataType())?, usizes(&array.shape())?, usizes(&array.strides())?) };
    let out: RefCell<Result<Vec<f32>>> = RefCell::new(Err(Error::Output("Core ML gave no buffer".into())));
    let handler = StackBlock::new(|ptr: NonNull<c_void>, size: NSInteger| {
        let Ok(size) = usize::try_from(size) else { return };
        // SAFETY: Core ML's buffer is `size` bytes, readable for the handler's duration; `gather` reads only inside it.
        let bytes = unsafe { std::slice::from_raw_parts(ptr.as_ptr().cast::<u8>(), size) };
        *out.borrow_mut() = gather(&shape, &strides, elem, bytes);
    });
    // SAFETY: checked in `available`; the handler runs synchronously, before this call returns.
    unsafe { array.getBytesWithHandler(&handler) };
    let data = out.into_inner()?;
    Ok(Output { shape, data })
}

pub(crate) fn predict(handle: &Handle, input: &str, shape: &[usize], data: &[f32], output: &str) -> Result<Output> {
    autoreleasepool(|_| {
        let array = multi_array(shape, data)?;
        // SAFETY: wraps a valid multi-array (checked in `available`); never nil.
        let value = unsafe { MLFeatureValue::featureValueWithMultiArray(&array) };
        let key = NSString::from_str(input);
        let dict: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&[&*key], &[value.as_ref()]);
        let mut err: Option<Retained<NSError>> = None;
        // SAFETY: `initWithDictionary:error:` (checked in `available`) with string keys and `MLFeatureValue` values, as
        // documented; received as optional.
        let provider: Option<Retained<MLDictionaryFeatureProvider>> =
            unsafe { msg_send![MLDictionaryFeatureProvider::alloc(), initWithDictionary: &*dict, error: &mut err] };
        let provider = provider.ok_or_else(|| Error::Input(describe(err)))?;
        let provider: &ProtocolObject<dyn MLFeatureProvider> = ProtocolObject::from_ref(&*provider);
        let mut err: Option<Retained<NSError>> = None;
        // SAFETY: checked in `available`; the input names were checked against the model's description; received as
        // optional with an NSError out-parameter.
        let result: Option<Retained<ProtocolObject<dyn MLFeatureProvider>>> =
            unsafe { msg_send![&*handle.0, predictionFromFeatures: provider, error: &mut err] };
        let result = result.ok_or_else(|| Error::Predict(describe(err)))?;
        let name = NSString::from_str(output);
        // SAFETY: `featureValueForName:` is required by the MLFeatureProvider protocol; nil for an unknown name.
        let value: Option<Retained<MLFeatureValue>> = unsafe { msg_send![&*result, featureValueForName: &*name] };
        let value = value.ok_or_else(|| Error::Output(format!("the result has no `{output}`")))?;
        // SAFETY: a getter that returns nil when the value is not a multi-array; received as optional.
        let array: Option<Retained<MLMultiArray>> = unsafe { msg_send![&*value, multiArrayValue] };
        let array = array.ok_or_else(|| Error::Output(format!("`{output}` is not a tensor")))?;
        read(&array)
    })
}
