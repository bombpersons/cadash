"""Fireball shot of the tier-2 weapons (Blade of Fire / Staff of Fire), cartridge 0xB692/0xB7B4.
Writes game/assets/sprites/magic/weapon_shot.png + .sheet.json (anims "left", "right")."""
import os, sys, json
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from md import *
from lzss import rom
from sprites import mapping_pieces, render
import objects as O
from player import player_palette

r = rom()
OUT = os.path.join(HERE, '..', 'game', 'assets', 'sprites', 'magic')
vram = O.common_vram()
pal = [cram_rgb(w) for w in player_palette({'AC0': 0, 'AC2': 0, 'AC4': 0, 'AC6': 2, 'AC8': 0, 'ACA': 2})] + [(0, 0, 0)] * 32
frames, imgs = [], []
for side, f in (('left', 0), ('right', 1)):
    a = be32(r, 0xEA1E + 4 * f)
    mp = be32(r, a + 10)
    hdr, pcs = mapping_pieces(mp)
    img, ox, oy = render(pcs, vram, pal)
    imgs.append(img)
    frames.append({'origin': [ox, oy], 'boxes': hdr, 'side': side})
W = sum(i.width for i in imgs) + 2; H = max(i.height for i in imgs)
from PIL import Image
atlas = Image.new('RGBA', (W, H), (0, 0, 0, 0)); x = 0
for i, f in zip(imgs, frames):
    atlas.paste(i, (x, 0)); f['rect'] = [x, 0, i.width, i.height]; x += i.width + 1
atlas.save(os.path.join(OUT, 'weapon_shot.png'))
json.dump({'atlas': 'weapon_shot.png',
           'frames': [{'rect': f['rect'], 'origin': f['origin'], 'boxes': f['boxes']} for f in frames],
           'anims': {f['side']: {'loop': 0, 'frames': [{'frame': k, 'dur': 65535}]} for k, f in enumerate(frames)}},
          open(os.path.join(OUT, 'weapon_shot.sheet.json'), 'w'), indent=1)
print(frames)
