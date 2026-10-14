"""Export player (Fighter / Mage) sprites + animations from ROM data.
Output: game/assets/sprites/player/<class>.png (atlas) and <class>.json
"""
import sys, json, os
sys.path.insert(0, os.path.dirname(__file__))
from md import *
from lzss import rom, decompress
from sprites import mapping_pieces, stage_tiles, render, render_indexed
from anims import parse_block
import objects as O
from PIL import Image

r = rom()
OUT = os.path.join(os.path.dirname(__file__), '..', 'game', 'assets', 'sprites', 'player')

def words(a, n): return [be16(r, a + 2 * i) for i in range(n)]

def player_palette(gear):
    """Lines 0 and 1 as built by 0xC364 (gear = dict AC0..ACA indices)."""
    pal = [0] * 32
    pal[1] = 0x6AE
    pal[2:4] = words(0xB882, 2)
    pal[4:7] = words(0xB868 + 6 * gear['AC8'], 3)
    pal[7:9] = words(0xB87A, 2)
    pal[9:14] = words(0xB814 + 10 * gear['AC0'], 5)
    pal[15] = 0xEEE
    pal[17:19] = words(0xB88A, 2)
    pal[19:21] = words(0xB84C + 4 * gear['AC2'], 2)
    pal[21:23] = words(0xB83C + 4 * gear['AC4'], 2)
    pal[23:25] = words(0xB800 + 4 * gear['AC6'], 2)
    pal[25:27] = words(0xB858 + 4 * gear['ACA'], 2)
    pal[31] = 0xEEE
    return pal

def equipment_tiles(w, a, h, s):
    """Equipment tile set (cartridge 0x920C..0x95A4, LZSS bank 0x68C90) for a graphics variant:
    w = weapon variant (ACA 0,2 -> 0; 1,3 -> 1), a = AC2 (0..2), h = AC4 != 0, s = AC6 variant (0,3 -> 0; 1,2,4 -> 1)."""
    eq = bytearray(decompress(r, 0x68C90, 0xBF0)[:0x2000])
    # RAM past the 0x2000-byte bank holds font-bank bytes 0x1000..0x1420 (copied at 0x9238);
    # the larger swords (offset 0x1F20) read into it.
    eq += decompress(r, 0x639C4, 0xF8C)[0x1000:0x1420]
    v = bytearray(0x2000)
    def dma(off, words_, dst): v[dst:dst + words_ * 2] = eq[off:off + words_ * 2]
    dma(0x740 if w else 0x0, 0x3A0, 0x440)
    dma([0xE80, 0x11C0, 0x1500][a], 0x1A0, 0xF60)
    if h: dma(0x1840, 0xF0, 0x12A0)
    dma(0x1F20 if s else 0x1A20, 0x280, 0x1480)
    return v

def gear_variant(gear):
    return (gear['ACA'] & 1, min(gear['AC2'], 2), 1 if gear['AC4'] else 0, 0 if gear['AC6'] in (0, 3) else 1)

def equipment_vram(gear):
    return equipment_tiles(*gear_variant(gear))

# VRAM destination of each class's per-frame tile staging (staging slot = player +106 = class).
STAGING_VRAM = {'fighter': 0xB80, 'mage': 0x0000}

CLASSES = {
    # name: (anim block start, end, state table, body bank streams)
    'fighter': (0xEB1A, 0xEF32, 0xEA7E, [(0x66788, 0x128A), (0x67A12, 0x127E)]),
    'mage': (0xCD62, 0xD17A, 0xCCC6, [(0x66788, 0x128A), (0x67A12, 0x127E)]),
}
STATE_NAMES = ['stand', 'walk', 'crouch', 'jump', 'fall', 'state5', 'state6', 'state7', 'state8']
# Animation names by animation-pair address (offset inside each class block is identical).
ANIM_NAMES = {
    0x00: 'stand', 0x2C: 'walk', 0x94: 'crouch', 0xC0: 'jump', 0xEC: 'climb', 0x118: 'climb_move',
    0x158: 'hurt', 0x184: 'small', 0x1B0: 'small_walk', 0x1F0: 'attack_up', 0x244: 'attack_up_crouch',
    0x298: 'attack_up_jump', 0x2EC: 'attack', 0x32C: 'attack_crouch', 0x36C: 'attack_jump',
    0x3AC: 'attack_climb', 0x3EC: 'down_thrust',
}

def export(cls, gear):
    start, end, table, banks = CLASSES[cls]
    bank = b''.join(decompress(r, s, l)[:0x2000] for s, l in banks)
    bank += decompress(r, 0x639C4, 0xF8C)[:0x2000]  # tiles >= 0x200 come from the font bank at 0xFFADA4
    pal16 = player_palette(gear)
    pal = [cram_rgb(c) for c in pal16] + [(0, 0, 0)] * 32
    eqv = equipment_vram(gear)
    anims, _ = parse_block(start, end)
    frame_index = {}
    frames = []
    common = O.common_vram()
    def frame_vram(tl, eq_tiles):
        if tl is None:  # object-style frame (death burst): shared sprite tiles only
            return common
        vram = bytearray(0x10000)
        vram[0:0x2000] = eq_tiles
        st = stage_tiles(tl, bank)
        vram[STAGING_VRAM[cls]:STAGING_VRAM[cls] + len(st)] = st
        return vram
    def get_frame(tl, mp):
        key = (tl, mp)
        if key in frame_index: return frame_index[key]
        vram = frame_vram(tl, eqv)
        hdr, pcs = mapping_pieces(mp)
        img, ox, oy = render(pcs, vram, pal)
        frame_index[key] = len(frames)
        frames.append({'img': img, 'origin': [ox, oy], 'boxes': hdr, 'tiles': hex(tl) if tl is not None else None, 'map': hex(mp)})
        return frame_index[key]
    anim_out = {}
    for p, hr, fr, hl, fl in anims:
        for side, h, f in (('l', hr, fr), ('r', hl, fl)):  # pairs are [facing left, facing right]
            nm = ANIM_NAMES.get(p - start, f'{p:05x}')
            anim_out[f'{nm}_{side}'] = {
                'loop': h[3], 'collision_w': h[0], 'collision_h': h[1],
                'frames': [{'frame': get_frame(t, m), 'dur': d} for d, t, m in f],
            }
    # Death burst (0xA1B2: anim 0xB88E[facing], frames {dur, mapping}).
    for side, f in (('l', 0), ('r', 1)):
        a = be32(r, 0xB88E + 4 * f)
        last, loop = be16(r, a + 4), be16(r, a + 6)
        anim_out[f'dying_{side}'] = {'loop': loop, 'collision_w': be16(r, a), 'collision_h': be16(r, a + 2),
            'frames': [{'frame': get_frame(None, be32(r, a + 10 + 6 * k)), 'dur': be16(r, a + 8 + 6 * k)} for k in range(last + 1)]}
    # state table: 9 states x 4 slots of pair pointers
    states = []
    for s in range(9):
        grp = be32(r, table + 4 * s)
        slots = []
        for k in range(4):
            pp = be32(r, grp + 4 * k)
            slots.append(None if pp == 0 else ANIM_NAMES.get(pp - start, f'{pp:05x}'))
        states.append({'name': STATE_NAMES[s], 'slots': slots})
    # atlas
    pad = 1
    x = y = rowh = 0; W = 1024
    rects = []
    for f in frames:
        img = f['img']; w, h = (img.size if img else (1, 1))
        if x + w + pad > W: x = 0; y += rowh + pad; rowh = 0
        rects.append([x, y, w, h]); x += w + pad; rowh = max(rowh, h)
    atlas = Image.new('RGBA', (W, y + rowh), (0, 0, 0, 0))
    for f, rc in zip(frames, rects):
        if f['img']: atlas.paste(f['img'], (rc[0], rc[1]))
    os.makedirs(OUT, exist_ok=True)
    atlas.save(os.path.join(OUT, f'{cls}.png'))
    # Palette-indexed atlases for every equipment graphics variant (coloured at run time).
    variants = []
    for w in (0, 1):
        for a in (0, 1, 2):
            for hh in (0, 1):
                for sv in (0, 1):
                    eqv = equipment_tiles(w, a, hh, sv)
                    idx = Image.new('L', atlas.size, 0)
                    for (tl, mp), fi in frame_index.items():
                        vram = frame_vram(tl, eqv)
                        img, _, _ = render_indexed(mapping_pieces(mp)[1], vram)
                        if img: idx.paste(img, (rects[fi][0], rects[fi][1]))
                    name = f'{cls}_eq{w}{a}{hh}{sv}.png'
                    idx.save(os.path.join(OUT, name))
                    variants.append(name)
    json_name = f'{cls}.sheet.json'
    meta = {
        'atlas': f'{cls}.png',
        'frames': [{'rect': rc, 'origin': f['origin'], 'boxes': f['boxes']} for f, rc in zip(frames, rects)],
        'anims': anim_out,
        'states': states,
        'gear': gear,
        'indexed_variants': variants,
    }
    json.dump(meta, open(os.path.join(OUT, json_name), 'w'), indent=1)
    print(cls, len(frames), 'frames', len(anim_out), 'anims', atlas.size)

def palette_tables():
    """Tables used by the player palette builder (cartridge 0xC364)."""
    t = {
        'fixed_1': 0x6AE, 'b882': words(0xB882, 2), 'b87a': words(0xB87A, 2), 'b88a': words(0xB88A, 2),
        'ac8': [words(0xB868 + 6 * i, 3) for i in range(3)],
        'ac0': [words(0xB814 + 10 * i, 5) for i in range(4)],
        'ac2': [words(0xB84C + 4 * i, 2) for i in range(3)],
        'ac4': [words(0xB83C + 4 * i, 2) for i in range(4)],
        'ac6': [words(0xB800 + 4 * i, 2) for i in range(5)],
        'aca': [words(0xB858 + 4 * i, 2) for i in range(4)],
        'ladder': list(LADDER), 'ladder_g': list(LADDER_G),
        # Poison tint (0xC2D6): Fighter line0[7..8] (and [12..13] without armour) use 0xB87E; Mage line0[2..3] use 0xB886.
        'poison_fighter': words(0xB87E, 2), 'poison_mage': words(0xB886, 2),
    }
    d = os.path.join(os.path.dirname(__file__), '..', 'game', 'assets', 'data')
    json.dump(t, open(os.path.join(d, 'player.palette.json'), 'w'), indent=1)

if __name__ == '__main__':
    palette_tables()
    gear = {'AC0': 0, 'AC2': 0, 'AC4': 0, 'AC6': 0, 'AC8': 0, 'ACA': 0}
    export('fighter', gear)
    export('mage', gear)
