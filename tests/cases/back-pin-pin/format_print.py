#!/usr/bin/env python3
import json, os, sys

if len(sys.argv) > 1:
    path = sys.argv[1]
else:
    path = os.path.join(os.path.dirname(__file__), "animation_values.json")

with open(path) as f:
    data = json.load(f)

print("-" * 150)
for fr in data["frames"]:
    fi = fr["frame_index"]
    st = fr["scene_time_secs"]
    val = fr["values"]["FloatInput_53:value"]
    slt = fr["state_local_times"]
    parts = "  ".join(f"{k}: {v:>5.2f}" for k, v in slt.items())
    print(f"frame: {fi:>3d}    scene_time: {st:>5.2f}   value: {val:>5.2f}   {parts}")
