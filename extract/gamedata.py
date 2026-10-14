"""Exports physics/level data tables to game/assets/data/physics.data.json."""
import sys, os, json, struct
sys.path.insert(0, os.path.dirname(__file__))
from md import *; from lzss import rom
r = rom()
OUT = os.path.join(os.path.dirname(__file__), '..', 'game', 'assets', 'data')
# 0x36C4 is a table of 7 pointers to the 16-row masks (0x36E0, 0x3700, ...).
masks = [[be16(r, be32(r, 0x36C4 + 4 * n) + row * 2) for row in range(16)] for n in range(7)]
stairs = []
for n in range(12):
    a = 0x4992 + n * 6
    stairs.append({'dy': struct.unpack('>h', r[a:a + 2])[0], 'freeze': be16(r, a + 2), 'iframes': be16(r, a + 4)})
levels = {}
for cls, name in ((0, 'fighter'), (1, 'mage')):
    base = be32(r, 0xB896 + cls * 4)
    rows = []
    for i in range(20):
        a = base + i * 10
        rows.append({'exp': be16(r, a), 'hp': r[a + 2], 'mp': r[a + 3], 'atk': r[a + 4], 'def': r[a + 5],
                     'speed': r[a + 6], 'spells': r[a + 7], 'attack_frame': be16(r, a + 8)})
    levels[name] = list(reversed(rows))  # stored highest level first
idx = json.load(open(os.path.join(os.path.dirname(__file__), 'out', 'rooms', 'rooms_index.json')))
event_warps = {str(e['ff0a74']): {'room': e['dest_room'], 'x': e['dest_world_px'][0], 'y': e['dest_world_px'][1],
                                 'facing': e['dest_facing'], 'camera': [e['dest_camera_blocks'][0] * 16, e['dest_camera_blocks'][1] * 16],
                                 'music': e['music']} for e in idx['starts']['event_warps']}
def stream(a):
    out = []
    while not (r[a] == 0 and r[a + 1] == 0):
        out.append([r[a], r[a + 1]]); a += 2
    return out
# Recorded pad streams {ticks, buttons (bit7 Start, 6 A, 5 C, 4 B, 3 R, 2 L, 1 D, 0 U)}.
demos = {'ending': stream(0x632A), 'attract_p1': stream(0x4F2A), 'attract_p2': stream(0x592A)}
os.makedirs(OUT, exist_ok=True)
json.dump({'demos': demos, 'surface_masks': masks, 'stairs': stairs, 'levels': levels, 'event_warps': event_warps}, open(os.path.join(OUT, 'physics.data.json'), 'w'), indent=1)
print(stairs[:3]); print(levels['fighter'][0], levels['fighter'][-1]); print(levels['mage'][0])
