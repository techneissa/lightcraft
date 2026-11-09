//! Portrait retouching over MCP (fork, `fork/retouch-foundation.md`): its values are develop controls, and its
//! commands are tools.

use serde_json::json;

use crate::{Headless, call_tool};

#[test]
fn mcp_reads_and_sets_a_portrait_value() {
    let mut b = Headless::demo();
    let first = b.session.visible_cloned()[0];
    // listed with the develop controls, in their own section
    let r = call_tool(&mut b, "list_controls", &json!({"section": "portrait"}));
    assert!(!r.is_error, "{r:?}");
    let listed = r.structured.clone().unwrap_or_else(|| serde_json::from_str(r.content[0]["text"].as_str().unwrap()).unwrap());
    let skin = listed.as_array().unwrap().iter().find(|c| c["id"] == "retouch.skin.amount").cloned().unwrap_or_default();
    assert_eq!((skin["value"].clone(), skin["section"].clone()), (json!(0.0), json!("portrait")), "{listed}");
    // set like any slider
    let r = call_tool(&mut b, "set_develop", &json!({"id": first.0, "values": {"retouch.skin.amount": 35}}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(b.session.develop_of(first).unwrap().retouch.global.get("retouch.skin.amount"), Some(35.0));
    // read back in the settings JSON, under the fork's key
    let r = call_tool(&mut b, "get_develop", &json!({"id": first.0}));
    assert!(!r.is_error, "{r:?}");
    let d = r.structured.clone().unwrap_or_else(|| serde_json::from_str(r.content[0]["text"].as_str().unwrap()).unwrap());
    assert_eq!(d["fork_retouch"]["global"]["skin"]["amount"], json!(35.0), "{d}");
    // and through the retouch commands
    let r = call_tool(&mut b, &crate::command_tool_name("retouch.set"), &json!({"control": "retouch.eyes.whites", "value": 20}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(b.session.develop_of(first).unwrap().retouch.global.get("retouch.eyes.whites"), Some(20.0));
    let r = call_tool(&mut b, &crate::command_tool_name("retouch.faces"), &json!({}));
    assert!(!r.is_error, "{r:?}");
    assert!(call_tool(&mut b, "set_develop", &json!({"values": {"retouch.nope.amount": 1}})).is_error);
}
