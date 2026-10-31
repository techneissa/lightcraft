#!/usr/bin/env python3
# Writes the camera and lens NAME lists the Info panel's pickers search (names only, no correction data):
#
#   crates/lensdb/data/camera-names.tsv   maker <TAB> name <TAB> aliases (";"-separated)
#   crates/lensdb/data/lens-names.tsv     maker <TAB> name <TAB> mounts  (";"-separated)
#
#   git clone https://github.com/lensfun/lensfun ../lensfun
#   git -C ../lensfun checkout bc5615d6278cb5fb75dd53e74015367453d5591d
#   python3 tools/camera_lens_names.py ../lensfun docs/raw/adobe-cameras.tsv crates/lensdb/data
#
# Sources: lensfun's data/db/*.xml (CC-BY-SA 3.0; the maintainer approved using the NAMES only: the lens and camera
# names and the lens mounts, none of the calibration data, and no lensfun code), and docs/raw/adobe-cameras.tsv (camera
# names as published by the camera vendors' support lists, facts only). Only the Python standard library is used.
#
# Clean-up: built-in / compact-camera entries ("... & compatibles") and generic placeholders are dropped; maker
# spellings are collapsed ("Nikon Corporation" -> "Nikon", "Olympus Optical Co.,Ltd" -> "Olympus", case variants);
# the untagged (else English) model name is kept; a lens listed once per crop factor is listed once with its mounts
# merged; a name is "Maker Model" unless the model already starts with the maker.
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

MAKERS = {
    "nikon corporation": "Nikon",
    "olympus optical co.,ltd": "Olympus",
    "olympus imaging corp.": "Olympus",
    "olympus corporation": "Olympus",
    "om digital solutions": "OM System",
    "leica camera ag": "Leica",
    "pentax corporation": "Pentax",
    "ricoh imaging company, ltd.": "Ricoh",
    "minolta co., ltd.": "Minolta",
    "casio computer co.,ltd": "Casio",
    "carl zeiss": "Zeiss",
    "kmz": "KMZ",
    "кмз": "KMZ",
    "voigtländer": "Voigtländer",
    "voigtlander": "Voigtländer",
    "generisch": "Generic",
    "samsung techwin": "Samsung",
    "eastman kodak company": "Kodak",
    "fujifilm corporation": "Fujifilm",
    "fuji photo film co., ltd.": "Fujifilm",
    "sony corporation": "Sony",
    "panasonic corporation": "Panasonic",
    "matsushita electric industrial co., ltd.": "Panasonic",
}
# Names that are placeholders, not products.
SKIP_MAKERS = {"generic"}
MAX_NAME = 120


def maker_of(text):
    text = " ".join((text or "").split())
    return MAKERS.get(text.lower(), text)


def pick(el, tag):
    """The untagged text of `tag`, else its English one."""
    plain = [e.text for e in el.findall(tag) if not e.get("lang") and e.text]
    en = [e.text for e in el.findall(tag) if e.get("lang") == "en" and e.text]
    return " ".join((plain or en or [""])[0].split())


def english(el, tag):
    """Names given in English beside the untagged one (marketing names)."""
    return [" ".join(e.text.split()) for e in el.findall(tag) if e.get("lang") == "en" and e.text]


def full_name(maker, model):
    first = maker.split()[0].lower() if maker else ""
    if not maker or model.lower().startswith(first):
        return model
    return f"{maker} {model}"


def key_of(name):
    return " ".join(name.lower().split())


def lensfun(db):
    cameras, lenses = {}, {}
    for f in sorted(Path(db).glob("*.xml")):
        root = ET.parse(f).getroot()
        for el in root.findall("camera"):
            maker, model = maker_of(pick(el, "maker")), pick(el, "model")
            if not model or "& compatibles" in model or "35mm film" in model or maker.lower() in SKIP_MAKERS:
                continue
            name = full_name(maker, model)
            if len(name) > MAX_NAME:
                continue
            e = cameras.setdefault(key_of(name), {"maker": maker, "name": name, "aliases": set()})
            for a in english(el, "model"):
                if "& compatibles" not in a and key_of(full_name(maker, a)) != key_of(name):
                    e["aliases"].add(a)
        for el in root.findall("lens"):
            maker, model = maker_of(pick(el, "maker")), pick(el, "model")
            if not model or "& compatibles" in model or maker.lower() in SKIP_MAKERS:
                continue
            name = full_name(maker, model)
            if len(name) > MAX_NAME:
                continue
            e = lenses.setdefault(key_of(name), {"maker": maker, "name": name, "mounts": set()})
            for m in el.findall("mount"):
                if m.text and m.text.strip().lower() != "generic":
                    e["mounts"].add(" ".join(m.text.split()))
    return cameras, lenses


SONY_CODES = ("ILCE-", "ILME-", "ILCA-", "DSC-", "DSLR-", "SLT-", "NEX-")


def adobe(path, cameras):
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#") or line.startswith("maker\t"):
            continue
        cols = line.split("\t")
        if len(cols) < 2:
            continue
        maker, name = maker_of(cols[0]), " ".join(cols[1].split())
        # trailing notes: "(** see note)"
        name = name.split("(*")[0].strip()
        alts = []
        if name.endswith(")") and "(" in name:
            head, _, tail = name[:-1].rpartition("(")
            if head.strip() and tail.strip():
                name, alts = head.strip(), [a.strip() for a in tail.split(",") if a.strip()]
        if maker == "Sony":
            # Sony's files say the body code (ILCE-7M5), the lists the marketing name (A7 V)
            codes = [a for a in alts if a.upper().startswith(SONY_CODES)]
            if codes:
                name, alts = codes[0], [name] + [a for a in alts if a != codes[0]]
        full = full_name(maker, name)
        if not name or len(full) > MAX_NAME:
            continue
        e = cameras.setdefault(key_of(full), {"maker": maker, "name": full, "aliases": set()})
        for a in alts:
            if key_of(a) != key_of(name):
                e["aliases"].add(a)


def write(path, rows, header, third):
    rows = sorted(rows, key=lambda r: (r["maker"].lower(), r["name"].lower()))
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(header)
        for r in rows:
            f.write(f"{r['maker']}\t{r['name']}\t{';'.join(sorted(r[third]))}\n")
    return len(rows)


def main():
    if len(sys.argv) != 4:
        sys.exit("usage: camera_lens_names.py <lensfun checkout> <adobe-cameras.tsv> <output dir>")
    db, adobe_tsv, out = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    cameras, lenses = lensfun(Path(db) / "data" / "db")
    adobe(adobe_tsv, cameras)
    out.mkdir(parents=True, exist_ok=True)
    note = (
        "# Names only. Camera names: lensfun (bc5615d6278cb5fb75dd53e74015367453d5591d, data/db, CC-BY-SA 3.0) merged with\n"
        "# docs/raw/adobe-cameras.tsv. Generated by tools/camera_lens_names.py; do not edit by hand.\n"
    )
    n = write(out / "camera-names.tsv", cameras.values(), note + "# maker\tname\taliases\n", "aliases")
    m = write(
        out / "lens-names.tsv",
        lenses.values(),
        note.replace("Camera names", "Lens names and mounts").replace(" merged with\n# docs/raw/adobe-cameras.tsv.", ".")
        + "# maker\tname\tmounts\n",
        "mounts",
    )
    print(f"{n} cameras, {m} lenses")


if __name__ == "__main__":
    main()
