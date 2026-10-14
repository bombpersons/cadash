"""Export the Mage's spell graphics + numeric spell data (see docs/re/magic.md).

Tiles: LZSS 0x64950 (len 0x1196) is DMA'd at game start (0x3EEC) to VRAM 0x5600 => VRAM tile t = bank tile t-0x2B0.
All spell/icon sprite pieces use palette line 1; they only use colours 11..15. Colours 11..13 are written by the
selection icon (0xAB4C) from table 0xE3CA + 6*spell, 14 = 0x000, 15 = 0xEEE (player palette, 0xC364).

Outputs: game/assets/sprites/magic/<spell>.png + <spell>.sheet.json, icon.png + icon.sheet.json,
         game/assets/data/magic.magic.json
"""
import sys, os, json, struct
sys.path.insert(0, os.path.dirname(__file__))
from md import *
from lzss import rom, decompress
from sprites import mapping_pieces, render
from PIL import Image

r = rom()
ROOT = os.path.join(os.path.dirname(__file__), '..')
OUT = os.path.join(ROOT, 'game', 'assets', 'sprites', 'magic')
DATA = os.path.join(ROOT, 'game', 'assets', 'data', 'magic.magic.json')

def W(a): return be16(r, a)
def S(a): return struct.unpack('>h', r[a:a + 2])[0]
def L(a): return be32(r, a)
def SL(a): return struct.unpack('>i', r[a:a + 4])[0]

BANK = decompress(r, 0x64950, 0x1196)
VRAM = bytearray(0x10000)
VRAM[0x5600:0x5600 + 0x1A00] = BANK[:0x1A00]          # DMA at 0x3F20: 0xD00 words -> VRAM 0x5600

SPELLS = ['basic_magic', 'flying_dagger', 'flame_wall', 'blizzard', 'lightning_bolt', 'explosion']
COST = [W(0xAB40 + 2 * i) for i in range(6)]
ICON_PAL = [[W(0xE3CA + 6 * s + 2 * i) for i in range(3)] for s in range(6)]

def palette(spell):
    line1 = [0] * 16
    line1[11:14] = ICON_PAL[spell]
    line1[15] = 0xEEE
    pal = [(0, 0, 0)] * 64
    for i, c in enumerate(line1): pal[16 + i] = cram_rgb(c)
    return pal

def anim(a):
    """object anim descriptor: w coll_w(+62), w coll_h(+64), w last, w loop, then {w dur, l mapping} * (last+1)"""
    w, h, last, loop = (W(a + 2 * i) for i in range(4))
    return {'size': [w, h], 'loop': loop, 'frames': [(W(a + 8 + 6 * i), L(a + 10 + 6 * i)) for i in range(last + 1)]}

class Sheet:
    def __init__(self, name):
        self.name = name; self.frames = []; self.index = {}; self.anims = {}
    def frame(self, mp, spell):
        key = (mp, spell)
        if key not in self.index:
            hdr, pcs = mapping_pieces(mp)
            img, ox, oy = render(pcs, VRAM, palette(spell))
            self.index[key] = len(self.frames)
            self.frames.append({'img': img, 'origin': [ox, oy], 'boxes': hdr, 'mapping': '0x%X' % mp})
        return self.index[key]
    def write(self):
        pad = 1; x = 0; H = max(f['img'].height for f in self.frames)
        atlas = Image.new('RGBA', (sum(f['img'].width + pad for f in self.frames), H), (0, 0, 0, 0))
        out = []
        for f in self.frames:
            atlas.paste(f['img'], (x, 0))
            out.append({'rect': [x, 0, f['img'].width, f['img'].height], 'origin': f['origin'], 'boxes': f['boxes']})
            x += f['img'].width + pad
        atlas.save(os.path.join(OUT, self.name + '.png'))
        meta = {'atlas': self.name + '.png', 'frames': out, 'anims': self.anims}
        json.dump(meta, open(os.path.join(OUT, self.name + '.sheet.json'), 'w'), indent=1)
        return [f['mapping'] for f in self.frames]

def add_anim(sh, name, a, spell):
    d = anim(a)
    sh.anims[name] = {'loop': d['loop'], 'frames': [{'frame': sh.frame(mp, spell), 'dur': dur} for dur, mp in d['frames']]}
    return d

def main():
    os.makedirs(OUT, exist_ok=True)
    maps = {}
    # 0 basic magic: anim 0xE3EE[facing] (0 = left, 1 = right)
    sh = Sheet('basic_magic')
    add_anim(sh, 'left', L(0xE3EE), 0); add_anim(sh, 'right', L(0xE3F2), 0)
    maps['basic_magic'] = sh.write()
    # 1 flying dagger: anim 0xE3F6
    sh = Sheet('flying_dagger'); add_anim(sh, 'spin', 0xE3F6, 1); maps['flying_dagger'] = sh.write()
    # 2 flame wall: anim 0xE42E (object removed when the animation wraps)
    sh = Sheet('flame_wall'); add_anim(sh, 'burn', 0xE42E, 2); maps['flame_wall'] = sh.write()
    # 3 blizzard: mapping 0xE4C6[angle 0..31], set directly (no animation)
    sh = Sheet('blizzard')
    for k in range(32):
        sh.anims['angle_%d' % k] = {'loop': 0, 'frames': [{'frame': sh.frame(L(0xE4C6 + 4 * k), 3), 'dur': 0xFFFF}]}
    maps['blizzard'] = sh.write()
    # 4 lightning bolt: anim 0xE4A2[dir 0..8]
    sh = Sheet('lightning_bolt')
    for dnr in range(9): add_anim(sh, 'dir_%d' % dnr, L(0xE4A2 + 4 * dnr), 4)
    maps['lightning_bolt'] = sh.write()
    # selection icon: mapping 0xE2A6[spell], palette per spell
    sh = Sheet('icon')
    for s in range(6):
        sh.anims['spell_%d' % s] = {'loop': 0, 'frames': [{'frame': sh.frame(L(0xE2A6 + 4 * s), s), 'dur': 0xFFFF}]}
    maps['icon'] = sh.write()
    write_data(maps)

def write_data(maps):
    vel8 = [[SL(0xB310 + 8 * i) / 65536, SL(0xB314 + 8 * i) / 65536] for i in range(8)]
    bolt_dir = [[S(0xB5CA + 4 * i), S(0xB5CC + 4 * i)] for i in range(9)]
    turn = [list(r[L(0xCC4A + 4 * i):L(0xCC4A + 4 * i) + 9]) for i in range(9)]
    rings = []
    for i in range(30):
        p = L(0xBA2E + 4 * i)
        rings.append([list(struct.unpack('bb', r[p + 2 * k:p + 2 * k + 2])) for k in range(32)])
    lv = L(0xB89A)
    learned = {}
    for i in range(20):
        lvl = 20 - i; n = r[lv + 10 * i + 7]
        learned.setdefault(n, lvl); learned[n] = min(learned[n], lvl)
    msgs = [W(0xC7AA + 2 * i) for i in range(5)]
    def sp(i, **kw):
        d = {'index': i, 'name': SPELLS[i], 'mp_cost': COST[i], 'learned_at_level': learned.get(i, None),
             'icon_palette_colours_11_13': ['0x%03X' % c for c in ICON_PAL[i]],
             'icon_palette_rgb': [list(cram_rgb(c)) for c in ICON_PAL[i]]}
        d.update(kw); return d
    data = {
        'source': 'docs/re/magic.md; extract/magic.py',
        'units': 'positions/velocities in pixels and pixels/frame (16.16 fixed in the cartridge), times in 60 Hz frames; '
                 'offsets relative to the caster position (centre x, feet y); facing +1 = right, -1 = left',
        'charge': {
            'hold_frames_to_icon': 30, 'cycle_frames': 30,
            'first_cycle_after_icon': 29,
            'icon_offset': [8, -56], 'icon_sound': 0xB3,
            'cast_delay_frames': 10,
            'not_enough_mp_hud_message': 5,
            'learn_hud_messages': {str(i + 1): msgs[i] for i in range(5)},
            'learn_rule': 'level-up (0xC5EC): spells_known = level_table[level].byte7 (Mage: L1-2 0, L3 1, L6 2, L8 3, '
                          'L16 4, L20 5); when it changes, player+0x9C = value and HUD message 0xC7AA[value-1] is queued',
            'hud_message_text': {'5': "YOU DON'T HAVE\nENOUGH MP LEFT.",
                                 '6': "FLYING DAGGER'S \nSPELL HAS BEEN\n{PAGE}\nMEMORIZED.",
                                 '7': "FLAME WALL'S\nSPELL HAS BEEN\n{PAGE}\nMEMORIZED.",
                                 '8': "BLIZZARD'S \nSPELL HAS BEEN\n{PAGE}\nMEMORIZED.",
                                 '9': "LIGHTNING BOLT'S\nSPELL HAS BEEN\n{PAGE}\nMEMORIZED.",
                                 '10': "EXPLOSION'S\nSPELL HAS BEEN\n{PAGE}\nMEMORIZED."},
            'player_anim_on_cast': 'normal attack animation for the current state (substate 2, 0xA8C6); walking -> vx=vy=0',
            'hit_checks_on_odd_frames_only': True,
            'icon_sheet': 'sprites/magic/icon.sheet.json', 'icon_anims': ['spell_%d' % s for s in range(6)],
        },
        'spawn_point': {'dx_facing': 48, 'dy_default': -36, 'dy_crouch': -18, 'dy_climb': -26,
                        'note': '0xB62C: x = caster.x + 48*facing, y = caster.y - (36 | 18 if state 2 crouch | 26 if state 5/6 climbing)'},
        'offscreen_kill_screen_x': [-16, 336],
        'hit': {'enemy_invuln_frames': 16, 'sound_hit': 0xA7, 'sound_kill': 0xA8, 'sound_magic_proof': 0xA5,
                'consumed_on_hit_spells': [0, 1], 'enemy_hitstun_frames': 13},
        'spells': [
            sp(0, damage=20, sound=0xA6, projectiles=1, sheet='sprites/magic/basic_magic.sheet.json',
               anims={'left': 'left', 'right': 'right'}, velocity=[6.0, 0.0], collision_size=[16, 16],
               ends='hit / wall (not in room 24) / screen x outside [-16,336)'),
            sp(1, damage=40, sound=0xA6, projectiles=8, sheet='sprites/magic/flying_dagger.sheet.json', anims={'all': 'spin'},
               velocity_table_0xB310=vel8, velocity_table_order='object k (0..7) uses entry 7-k',
               accel=0x7000 / 65536, accel_every_frames=2, max_speed=7.0, home_target_offset=[0, -32], lifetime=180),
            sp(2, damage=20, sound=0xA6, sheet='sprites/magic/flame_wall.sheet.json', anims={'all': 'burn'},
               first_offset_x=24, step_x=16, spawn_interval=8, max_fires=15, slots=8, fire_lifetime=72,
               floor_probe_halfwidth=8, floor_probe_steps=3, floor_probe_step=16, max_pushups=5, pushup_step=8,
               requires_ground=True),
            sp(3, damage=40, sound=0xA6, projectiles=8, sheet='sprites/magic/blizzard.sheet.json',
               anims={'angle_k': ['angle_%d' % k for k in range(32)]},
               angle_start='object k (0..7) starts at angle 4*(7-k)', update_every_frames=2, center_drift_x=2,
               max_ring=29, lifetime=180, ring_table_0xBA2E=rings,
               mapping_by_angle=[L(0xE4C6 + 4 * k) for k in range(32)],
               hitbox_note='the cartridge never copies the mapping boxes for blizzard shards (stale/zero boxes => point test)'),
            sp(4, damage=20, sound=0xAC, projectiles=16, chains=2, segments_per_chain=8,
               sheet='sprites/magic/lightning_bolt.sheet.json', anims={'dir_d': ['dir_%d' % d for d in range(9)]},
               dir_velocity_0xB5CA=bolt_dir, dir_stop=4, initial_dir={'right': 1, 'left': 7}, deadzone=4,
               turn_table_0xCC4A=turn, lifetime=300, targets_considered=8),
            sp(5, damage=100, sound=0x9B, projectiles=1, sheet=None, anims={},
               note='no sprite; hits every active enemy once (see magic.md: stale damage quirk)'),
        ],
        'sheet_mappings': maps,
    }
    json.dump(data, open(DATA, 'w'), indent=1)

if __name__ == '__main__':
    main()
