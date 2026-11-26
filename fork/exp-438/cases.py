import json, math, random, sys
sp = sys.argv[1]
samples = {"a7v": [0.677, -0.004, -0.024], "x9": [0.615, 0.003, 0.004], "s6": [0.583, 0.037, 0.087], "p5": [0.241, 0.002, 0.023]}

def stroke(pts, size=0.14, flow=60.0, feather=100.0, erase=False):
    return {"points": [{"x": x, "y": y} for x, y in pts], "size": size, "feather": feather, "flow": flow, "density": 100.0, "erase": erase}

def line(a, b, n=24, wob=0.03, seed=0):
    r = random.Random(seed)
    return [(a[0] + (b[0]-a[0])*i/(n-1) + r.uniform(-wob, wob)*0.3, a[1] + (b[1]-a[1])*i/(n-1) + wob*math.sin(i/3)) for i in range(n)]

brush = [stroke(line((0.1, 0.2), (0.85, 0.25), seed=1)), stroke(line((0.15, 0.4), (0.8, 0.55), seed=2), flow=35.0),
         stroke(line((0.2, 0.75), (0.75, 0.6), seed=3), size=0.2, flow=20.0), stroke(line((0.3, 0.15), (0.35, 0.85), seed=4), size=0.1),
         stroke(line((0.6, 0.2), (0.7, 0.8), seed=5), flow=45.0), stroke(line((0.2, 0.5), (0.8, 0.35), seed=6), size=0.06, flow=40.0, erase=True)]

def shapes(photo):
    ai = json.load(open(f"{sp}/ai/{photo}.json"))[0]["components"][0]["shape"]
    return {
        "brush": {"kind": "brush", "strokes": brush},
        "linear": {"kind": "linear", "start": {"x": 0.5, "y": 0.02}, "end": {"x": 0.5, "y": 0.9}},
        "radial": {"kind": "radial", "center": {"x": 0.45, "y": 0.5}, "rx": 0.38, "ry": 0.3, "angle": 20.0, "feather": 85.0, "invert": False},
        "lumrange": {"kind": "luminanceRange", "lo": 0.35, "hi": 0.7, "lo_feather": 0.25, "hi_feather": 0.25},
        "colorrange": {"kind": "colorRange", "samples": [samples[photo]], "refine": 60.0},
        "ai": ai,
    }

def mask(i, name, shape, adjust, refine=0.0, decon=0.0):
    return {"id": i, "name": name, "visible": True, "invert": False, "on_photo": True,
            "components": [{"op": "add", "invert": False, "shape": shape}],
            "adjust": dict({"amount": 100.0}, **adjust), "refine": refine, "decontaminate": decon}

NORMAL = {"exposure": 0.6, "contrast": 15.0, "highlights": -30.0, "shadows": 25.0, "saturation": 15.0}
HARD = {"exposure": 4.0, "shadows": 100.0}
GLOBAL_HARD = {"light": {"shadows": 100.0}}

for photo in samples:
    s = shapes(photo)
    cases = []
    kinds = [("brush", s["brush"], 0, 0), ("linear", s["linear"], 0, 0), ("radial", s["radial"], 0, 0), ("lumrange", s["lumrange"], 0, 0),
             ("colorrange", s["colorrange"], 0, 0), ("ai", s["ai"], 0, 0), ("refine", s["brush"], 70.0, 0), ("decon", s["ai"], 0, 100.0)]
    for name, shape, refine, decon in kinds:
        cases.append({"name": f"{name}-normal", "settings": {"masks": [mask(1, name, shape, NORMAL, refine, decon)]}})
        cases.append({"name": f"{name}-hard", "settings": dict(GLOBAL_HARD, masks=[mask(1, name, shape, HARD, refine, decon)])})
    # all of them at once; hard: the gradient at +4 EV with shadows lifted, the others pushing both ways
    for edit in ("normal", "hard"):
        masks = []
        for i, (name, shape, refine, decon) in enumerate(kinds):
            if edit == "normal":
                adj = dict(NORMAL, exposure=[0.5, -0.4][i % 2])
            else:
                adj = HARD if name == "linear" else {"exposure": [1.5, -2.0][i % 2], "shadows": 60.0, "contrast": 40.0}
            masks.append(mask(i + 1, name, shape, adj, refine, decon))
        st = {"masks": masks} if edit == "normal" else dict(GLOBAL_HARD, masks=masks)
        cases.append({"name": f"all-{edit}", "settings": st})
    json.dump(cases, open(f"{sp}/cases/{photo}.json", "w"))
    print(photo, len(cases))
