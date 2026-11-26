# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow>=11", "pyarrow>=18", "numpy>=2"]
# ///
"""The mask scorer's public test sets (fork/mask-score.md): download them and convert the photos named in
`lists.json` into the scorer's folder format, `<data>/mask-score/<job>/<id>/{photo.png, mask.png, [ignore.png,]
meta.json}`. Nothing downloaded goes into the repository.

    uv run tools/mask-testsets/fetch.py [--data DIR] [--select]

`--data` defaults to `~/Library/Application Support/ArtCraft/datasets`. `--select` rebuilds `lists.json` from the
downloaded sets by the fixed rules below (only to change the sets: every baseline is then scored again).
"""

import argparse
import hashlib
import io
import json
import subprocess
import sys
import zipfile
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq
from PIL import Image, ImageCms, ImageDraw, ImageOps

LISTS = Path(__file__).resolve().with_name("lists.json")
LONG_SIDE = 2048
Image.MAX_IMAGE_PIXELS = 200_000_000

HF = "https://huggingface.co/datasets/{repo}/resolve/{rev}/{file}"
# (name: Hugging Face repo, pinned revision, parquet file, licence)
PARQUET = {
    "dis-vd": ("nobg/DIS5K", "e007e9a3af50dec16d0fca3c0856793d20b9a448", "data/DIS_VD-00000-of-00001-87c0cab31f89a0a6.parquet",
               "DIS5K Terms of Use (non-commercial research and education)"),
    "p3m-500-np": ("nobg/P3M-10K", "a239de9297bd56e372cf3c9c82f7085d59e71219", "data/P3M_500_NP-00000-of-00001.parquet",
                   "MIT (P3M-10k release agreement; photo copyright stays with the owners)"),
    "aim-500": ("nobg/AIM-500", "b7d6542c14d7dbb51e5649c8ce11934ee6289d10", "data/test-00000-of-00001.parquet", "MIT (AIM release agreement)"),
}
COCO_ANNOTATIONS = "http://images.cocodataset.org/annotations/annotations_trainval2017.zip"
COCO_IMAGE = "http://images.cocodataset.org/val2017/{:012d}.jpg"
STUFF_MAPS = "http://calvin.inf.ed.ac.uk/wp-content/uploads/data/cocostuffdataset/stuffthingmaps_trainval2017.zip"
COCO_LICENCE = "labels CC BY 4.0; photos under their owners' Flickr terms (evaluation only)"
# COCO-Stuff pixel values (category id - 1): clouds, sky-other; and unlabelled
SKY_VALUES = (105, 156)
UNLABELLED = 255


def download(url: str, path: Path) -> Path:
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        part = path.with_name(path.name + ".part")
        print(f"downloading {url}", flush=True)
        subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--silent", "--show-error", "-C", "-", "-o", str(part), url],
                       check=True)
        part.replace(path)
    return path


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def note_source(folder: Path, url: str, licence: str, file: Path) -> None:
    out = folder / "source.json"
    if out.exists():
        return
    with file.open("rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    out.write_text(json.dumps({"url": url, "licence": licence, "bytes": file.stat().st_size, "sha256": digest}, indent=2) + "\n")


def parquet(data: Path, name: str) -> list[dict]:
    repo, rev, file, licence = PARQUET[name]
    url = HF.format(repo=repo, rev=rev, file=file)
    path = download(url, data / name / Path(file).name)
    note_source(data / name, url, licence, path)
    return pq.read_table(path).to_pylist()


def coco_zip(data: Path, url: str, folder: str, licence: str) -> zipfile.ZipFile:
    path = download(url, data / folder / url.rsplit("/", 1)[1])
    note_source(data / folder, url, licence, path)
    return zipfile.ZipFile(path)


def coco_image(data: Path, image_id: int) -> bytes:
    return download(COCO_IMAGE.format(image_id), data / "coco-val2017" / "images" / f"{image_id:012d}.jpg").read_bytes()


def instances(data: Path) -> dict:
    z = coco_zip(data, COCO_ANNOTATIONS, "coco-val2017", COCO_LICENCE)
    return json.loads(z.read("annotations/instances_val2017.json"))


def srgb(img: Image.Image) -> Image.Image:
    icc = img.info.get("icc_profile")
    if icc:
        try:
            return ImageCms.profileToProfile(img, ImageCms.ImageCmsProfile(io.BytesIO(icc)), ImageCms.createProfile("sRGB"),
                                             outputMode="RGB")
        except (ImageCms.PyCMSError, OSError, ValueError):
            pass
    return img.convert("RGB")


def fitted(size: tuple[int, int]) -> tuple[int, int]:
    w, h = size
    k = min(1.0, LONG_SIDE / max(w, h))
    return max(1, round(w * k)), max(1, round(h * k))


def write(data: Path, job: str, ident: str, photo: bytes, truth: Image.Image, meta: dict, soft: bool,
          ignore: Image.Image | None = None) -> None:
    """One photo in the scorer's format. `truth` is grey, the photo's size; a soft truth keeps its alpha."""
    out = data / "mask-score" / job / ident
    if (out / "meta.json").exists():
        return
    out.mkdir(parents=True, exist_ok=True)
    img = Image.open(io.BytesIO(photo))
    # (turned upright only when the truth was drawn that way)
    upright = ImageOps.exif_transpose(img)
    if upright.size == truth.size:
        img = upright
    img = srgb(img)
    if img.size != truth.size:
        raise SystemExit(f"{job}/{ident}: photo {img.size} and truth {truth.size} differ")
    size = fitted(img.size)
    truth = truth.convert("L")
    if size != img.size:
        img = img.resize(size, Image.Resampling.LANCZOS)
        truth = truth.resize(size, Image.Resampling.BOX)
    if not soft:
        truth = truth.point(lambda v: 255 if v >= 128 else 0)
    img.save(out / "photo.png", compress_level=4)
    truth.save(out / "mask.png")
    if ignore is not None:
        ignore.resize(size, Image.Resampling.NEAREST).save(out / "ignore.png")
    # (written last: its presence says the folder is complete)
    meta = {"job": job, **meta, "source_sha256": sha256(photo)}
    (out / "meta.json").write_text(json.dumps(meta, indent=2) + "\n")


def by_hash(items, key):
    return sorted(items, key=lambda x: sha256(str(key(x)).encode()))


def select(data: Path) -> dict:
    """The fixed sets: every run scores the same photos."""
    dis = sorted(r["image_name"] for r in parquet(data, "dis-vd"))
    subject = [{"set": "dis-vd", "name": n} for n in dis[:: max(1, len(dis) // 100)][:100]]

    # (P3M's files carry no names: the row in the pinned file)
    p3m = len(parquet(data, "p3m-500-np"))
    aim = sorted(r["image_name"] for r in parquet(data, "aim-500"))
    hair = [{"set": "p3m-500-np", "row": i} for i in range(0, p3m, p3m // 100)][:100]
    hair += [{"set": "aim-500", "name": n} for n in aim[:: len(aim) // 50]][:50]

    coco = instances(data)
    images = {i["id"]: i for i in coco["images"]}
    per_image: dict[int, list] = {}
    for a in coco["annotations"]:
        img = images[a["image_id"]]
        share = a["area"] / (img["width"] * img["height"])
        if not a["iscrowd"] and isinstance(a["segmentation"], list) and 0.02 <= share <= 0.6:
            per_image.setdefault(a["image_id"], []).append(a)
    # one object a photo, then round the categories until there are 100
    chosen = [by_hash(v, lambda a: a["id"])[0] for v in per_image.values()]
    by_category: dict[int, list] = {}
    for a in by_hash(chosen, lambda a: a["id"]):
        by_category.setdefault(a["category_id"], []).append(a)
    click, round_ = [], 0
    while len(click) < 100 and any(len(v) > round_ for v in by_category.values()):
        click += [{"image": v[round_]["image_id"], "annotation": v[round_]["id"]}
                  for _, v in sorted(by_category.items()) if len(v) > round_]
        round_ += 1
    click = click[:100]

    sky = []
    maps = coco_zip(data, STUFF_MAPS, "coco-stuff-val2017", COCO_LICENCE)
    names = [n for n in maps.namelist() if n.startswith("val2017/") and n.endswith(".png")]
    for name in by_hash(names, lambda n: Path(n).stem):
        pixels = np.asarray(Image.open(io.BytesIO(maps.read(name))))
        share = np.isin(pixels, SKY_VALUES).mean()
        if 0.05 <= share <= 0.9:
            sky.append({"image": int(Path(name).stem)})
        if len(sky) == 100:
            break
    return {"subject": subject, "hair": hair, "click": click, "sky": sky}


def convert(data: Path, lists: dict) -> None:
    rows: dict[str, list] = {}

    def row(entry: dict) -> tuple[str, dict]:
        name = entry["set"]
        if name not in rows:
            rows[name] = parquet(data, name)
        if "row" in entry:
            return f"{name}-{entry['row']:03d}", rows[name][entry["row"]]
        found = next(r for r in rows[name] if r["image_name"] == entry["name"])
        return f"{name}-{entry['name']}".replace("#", "-").replace(" ", "-"), found

    for job in ("subject", "hair"):
        for k, entry in enumerate(lists[job]):
            ident, r = row(entry)
            # (DIS names are long and carry the category: a short id, the name kept in meta)
            if entry["set"] == "dis-vd":
                ident = f"dis-vd-{k:03d}"
            photo = r["image"]["bytes"]
            truth = Image.open(io.BytesIO((r.get("mask") or r.get("label"))["bytes"]))
            if truth.mode in ("RGBA", "LA"):
                truth = truth.convert("RGB")
            meta = {"source": entry, "licence": PARQUET[entry["set"]][3]}
            write(data, job, ident, photo, truth.convert("L"), meta, soft=job == "hair")
        print(f"{job}: {len(lists[job])} photos ready", flush=True)

    coco = instances(data)
    annotations = {a["id"]: a for a in coco["annotations"]}
    categories = {c["id"]: c["name"] for c in coco["categories"]}
    for entry in lists["click"]:
        a = annotations[entry["annotation"]]
        photo = coco_image(data, entry["image"])
        size = Image.open(io.BytesIO(photo)).size
        # (drawn four times larger and averaged down, so the outline is not biased by the rasteriser)
        big = Image.new("L", (size[0] * 4, size[1] * 4), 0)
        draw = ImageDraw.Draw(big)
        for polygon in a["segmentation"]:
            draw.polygon([v * 4 for v in polygon], fill=255)
        truth = big.resize(size, Image.Resampling.BOX)
        meta = {"source": entry, "category": categories[a["category_id"]], "licence": COCO_LICENCE}
        write(data, "click", f"coco-{entry['image']}-{entry['annotation']}", photo, truth, meta, soft=False)
    print(f"click: {len(lists['click'])} photos ready", flush=True)

    maps = coco_zip(data, STUFF_MAPS, "coco-stuff-val2017", COCO_LICENCE)
    for entry in lists["sky"]:
        pixels = np.asarray(Image.open(io.BytesIO(maps.read(f"val2017/{entry['image']:012d}.png"))))
        truth = Image.fromarray(np.where(np.isin(pixels, SKY_VALUES), 255, 0).astype(np.uint8))
        ignore = Image.fromarray(np.where(pixels == UNLABELLED, 255, 0).astype(np.uint8))
        meta = {"source": entry, "licence": COCO_LICENCE}
        write(data, "sky", f"coco-{entry['image']}", coco_image(data, entry["image"]), truth, meta, soft=False, ignore=ignore)
    print(f"sky: {len(lists['sky'])} photos ready", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--data", type=Path, default=Path.home() / "Library/Application Support/ArtCraft/datasets")
    parser.add_argument("--select", action="store_true", help="rebuild lists.json from the sets")
    args = parser.parse_args()
    if args.select:
        lists = select(args.data)
        lines = ",\n".join(
            f' "{job}": [\n' + ",\n".join(f"  {json.dumps(e)}" for e in entries) + "\n ]" for job, entries in lists.items())
        LISTS.write_text("{\n" + lines + "\n}\n")
    convert(args.data, json.loads(LISTS.read_text()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
