"""Extract every non-player object (enemies, bosses, NPCs, items, chests, projectiles) of Cadash.

Output (extract/out/objects/):
  opcodes.json                      semantics of every script opcode (B=behaviour, C=condition, I=state init)
  rooms.json                        per room: table addresses, spawn markers (cell + world px) resolved to records
  <group>/t<type>[_o<tile_ofs>]/    one directory per object type of a type-table group (+ tile offset variant)
        a<anim>_f<k>.png            every animation frame used by the type's states (RGBA)
        type.json                   states/script, spawn records (stats), animations (frames, durations,
                                    origin = object position inside the PNG, hit boxes, palette lines)
  sheets/<group>_t<type>.png        contact sheet of all frames (for eyeballing)

Usage: python3 extract/objects.py            (needs only the ROM; see docs/re/objects.md)
"""
import os, sys, json, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from objtables import *
from lzss import decompress
from md import cram_rgb, tile_pixels
from PIL import Image
import objops

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'out', 'objects')

# ------------------------------------------------------------------ graphics
def _dec(src, clen):
    b = bytearray(0x2800); d = decompress(r, src, clen); b[:len(d)] = d; return b

def common_vram():
    """VRAM sprite tiles loaded once at game start (routine ~0x3E16..0x402C)."""
    v = bytearray(0x10000)
    b = _dec(0x639C4, 0xF8C); v[0x5000:0x5600] = b[0x1A00:0x2000]; v[0x1980:0x1F80] = b[0xA00:0x1000]
    b = _dec(0x64950, 0x1196); v[0x5600:0x7000] = b[0:0x1A00]; v[0x1F80:0x2000] = b[0x1E00:0x1E80]
    b = _dec(0x7D5A2, 0x12D8); v[0x5200:0x5220] = b[0x1C00:0x1C20]
    return v

_vcache = {}
def room_vram(room):
    """common tiles + the per-room sprite bank list 0x2BA98[room] (routine 0x148B8).
    Each entry: LZSS (ptr,len) -> RAM 0xFF6DA4 (2000h bytes), DMA (ramsrc,len) -> VRAM dst.
    Entries reading past the 0x2000 decompressed bytes copy stale RAM (garbage) in the original; left 0 here."""
    a = L(T_BANKS + 4*room)
    if a in _vcache: return _vcache[a]
    v = common_vram()
    ram = bytearray(0x4000)
    for k in range(W(a)):
        q = a + 2 + 16*k
        d = decompress(r, L(q), W(q+4)); ram[:len(d)] = d
        off = L(q+6) - 0xFF6DA4; dst = L(q+10); n = W(q+14) * 2
        v[dst:dst+n] = ram[off:off+n]
    _vcache[a] = v
    return v

def player_palette(eq=(0,)*6):
    """palette lines 0/1 as built by 0xC364 from equipment indices (FF0AC0,AC2,AC4,AC6,AC8,ACA); default all 0"""
    ac0, ac2, ac4, ac6, ac8, aca = eq
    def ws(a, n): return [W(a + 2*i) for i in range(n)]
    l0 = [0, 0x6AE] + ws(0xB882, 2) + ws(0xB868 + 6*ac8, 3) + ws(0xB87A, 2) + ws(0xB814 + 10*ac0, 5) + [0, 0xEEE]
    l1 = [0] + ws(0xB88A, 2) + ws(0xB84C + 4*ac2, 2) + ws(0xB83C + 4*ac4, 2) + ws(0xB800 + 4*ac6, 2) + ws(0xB858 + 4*aca, 2) + [0]*4 + [0xEEE]
    return l0 + l1

def room_palette(room):
    p2 = L(T_PAL2 + 4*room); p3 = L(T_PAL3 + 4*room)
    words = player_palette() + [W(p2 + 2*i) for i in range(16)] + [W(p3 + 2*i) for i in range(16)]
    return [cram_rgb(w) for w in words], dict(line2=p2, line3=p3)

def render_mapping(mp, vram, pal, tile_ofs=0, base_attr=0):
    """returns (RGBA image, origin_x, origin_y, lines_used); origin = object (x,y) inside the image.
    VDP semantics: attr = base_attr | (piece attr + tile_ofs); w=((size>>2)&3)+1, h=(size&3)+1 cells, column-major,
    first piece drawn on top."""
    m = mapping(mp); pcs = m['pieces']
    if not pcs: return None, 0, 0, []
    boxes = []
    for p in pcs:
        w = ((p['size'] >> 2) & 3) + 1; h = (p['size'] & 3) + 1
        boxes.append((p['dx'], p['dy'], p['dx'] + 8*w, p['dy'] + 8*h))
    x0 = min(b[0] for b in boxes); y0 = min(b[1] for b in boxes)
    x1 = max(b[2] for b in boxes); y1 = max(b[3] for b in boxes)
    if x1 - x0 > 1024 or y1 - y0 > 1024: return None, 0, 0, []
    img = Image.new('RGBA', (x1 - x0, y1 - y0), (0, 0, 0, 0)); px = img.load()
    lines = set()
    for p in reversed(pcs):
        attr = base_attr | ((p['attr'] + tile_ofs) & 0xFFFF)
        w = ((p['size'] >> 2) & 3) + 1; h = (p['size'] & 3) + 1
        t0 = attr & 0x7FF; line = (attr >> 13) & 3; hf = attr & 0x800; vf = attr & 0x1000
        lines.add(line)
        for cx in range(w):
            for cy in range(h):
                tile = tile_pixels(vram, (t0 + cx*h + cy) & 0x7FF)
                tx = (w-1-cx) if hf else cx; ty = (h-1-cy) if vf else cy
                for y in range(8):
                    for x in range(8):
                        c = tile[7-y if vf else y][7-x if hf else x]
                        if c: px[p['dx'] - x0 + tx*8 + x, p['dy'] - y0 + ty*8 + y] = pal[line*16 + c] + (255,)
    return img, -x0, -y0, sorted(lines)

# ------------------------------------------------------------------ run-time graphics switches
# Some boss scripts reload the sprite bank at VRAM 0x7000 (0x2000 bytes) and palette line 2 (0x2E14 copy to
# 0xFF02B0/0xFF0330).  (lzss src, len, offset in the decompressed buffer, palette line 2 source); room=None: any room
GFX_SWITCH = {
    'I219': (0x7AF9A, 0x12B6, 0x000, 0x4C6A, None),   # Jelly Giant appears (msg 137)
    'B77':  (0x7AF9A, 0x12B6, 0xA00, 0x4B8A, 27),     # only in room 27 (at state_timer 22)
    'B101': (0x751BC, 0x13A0, 0x000, 0x4BEA, None),
    'B102': (0x78306, 0x17B8, 0x000, 0x4C0A, None),
    'I233': (0x76CB8, 0x164E, 0x000, 0x4ACA, None),
    'I21':  (None, None, None, 0x4B4A, None),          # palette only
}

def state_contexts(states, room, start='default'):
    """graphics context of every state: 'default' or the opcode that switched banks on the way there
    (start = context inherited from the spawning object)"""
    ctx = {}; todo = [(0, start)]
    while todo:
        s_, c = todo.pop()
        if s_ not in states: continue
        sc = states[s_]
        for op in ('I%d' % sc['init'], 'B%d' % sc['behaviour']):
            if op in GFX_SWITCH and GFX_SWITCH[op][4] in (None, room): c = op
        if s_ in ctx and (ctx[s_] == c or ctx[s_] != 'default'): continue
        ctx[s_] = c
        for _, n in sc['conds']: todo.append((n, c))
    for s_ in states: ctx.setdefault(s_, 'default')
    return ctx

def gfx_for(room, ctx):
    vram = room_vram(room); pal, palsrc = room_palette(room)
    if ctx == 'default': return vram, pal, palsrc
    src, ln, off, p2, _ = GFX_SWITCH[ctx]
    if src is not None:
        vram = bytearray(vram); buf = bytearray(0x4000); dd = decompress(r, src, ln); buf[:len(dd)] = dd
        vram[0x7000:0x9000] = buf[off:off + 0x2000]
    pal = list(pal); pal[32:48] = [cram_rgb(W(p2 + 2*i)) for i in range(16)]
    palsrc = dict(palsrc); palsrc['line2'] = p2
    return vram, pal, palsrc

# ------------------------------------------------------------------ names (hand-identified, see docs/re/objects.md)
COMMON_NAMES = {0: 'death_puff', 1: 'death_puff_large', 2: 'gold_bag', 3: 'item_medicinal_herb', 4: 'item_elixir',
                5: 'item_antidote', 6: 'item_mermaid_scale', 7: 'item_worm_silk', 8: 'item_lilliput_orchid', 9: 'item_gina_charm',
                10: 'item_abel_charm', 11: 'item_magic_flute', 12: 'item_golden_key', 13: 'death_puff_b', 14: 'hidden_item_spot',
                15: 'treasure_chest'}
NAMES = {}   # (type_table, type) -> name ; filled from docs knowledge
try:
    from objnames import NAMES as _N, GROUP_NAMES
    NAMES.update(_N)
except ImportError:
    GROUP_NAMES = {}

def type_name(tt, t):
    if (tt, t) in NAMES: return NAMES[(tt, t)]
    if t < 16: return COMMON_NAMES.get(t, 'common%d' % t)
    return 'type%d' % t

def group_name(tt):
    return GROUP_NAMES.get(tt, 'g%05x' % tt)

# ------------------------------------------------------------------ opcode descriptions
def opcode_table():
    auto = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'objops_auto.json'))) \
        if os.path.exists(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'objops_auto.json')) else {}
    out = {}
    for pre, base, n in (('B', BEHAVIOURS, N_BEH), ('C', CONDITIONS, N_COND), ('I', INITS, N_INIT)):
        for i in range(n):
            k = '%s%d' % (pre, i)
            out[k] = dict(addr='0x%X' % L(base + 4*i), desc=objops.MANUAL.get(k) or auto.get(k) or '?')
    return out

# ------------------------------------------------------------------ spawn relationships
# record indices (normal table of the current room) created by an opcode (extracted from the disassembly:
# move.w #n,d6 + bsr 0x13F16/0x1400E); room 27 adds 0x24 for B77/B81 (d5)
SPAWN_OPS = {"I190": [10, 11], "I195": [1, 2, 3, 4, 5, 6, 7, 8], "I216": [5], "I232": [28], "I234": [29, 30, 31, 32, 33],
    "B32": [5, 6, 7], "B33": [12], "B34": [13], "B36": [1], "B38": [3], "B39": [4], "B42": [7], "B43": [8], "B44": [3, 4, 5],
    "B46": [16, 17, 18], "B47": [16, 17], "B48": [18, 19], "B49": [9], "B50": [10], "B53": [14], "B54": [15], "B57": [1],
    "B58": [20, 21, 22, 23, 24], "B64": [1], "B65": [2, 3, 4, 5, 6, 7, 8, 9, 10], "B67": [1, 2], "B71": [3], "B73": [4],
    "B77": [1, 2, 3, 4, 5, 6, 7], "B81": [8, 9], "B88": [9, 10, 11, 12, 14, 15, 16, 17], "B89": [18], "B92": [29], "B95": [6],
    "B97": [26], "B98": [28, 29, 30, 31, 32, 33, 34, 35], "B108": [1, 2], "B109": [3, 16, 17, 18, 19, 20, 21, 22], "B111": [5],
    "B113": [7, 23, 24, 25, 26], "B127": [27], "B130": [35], "B131": [6, 34], "B133": list(range(36, 48))}

def spawn_ops(op, room):
    if op == 'B56': return sorted(set(r[0x17A34:0x17A74]))
    lst = SPAWN_OPS.get(op, [])
    if op in ('B77', 'B81') and room == 27: lst = [n + 0x24 for n in lst]
    return lst

# ------------------------------------------------------------------ main
def world_px(cx, cy):
    """marker cell -> spawn position. ROM coordinates carry a +0x80 offset (VDP sprite space):
    rom_x = cx*16 + 0x90; world pixel (map space) = cx*16 + 16 (bottom/right edge of the cell)."""
    return dict(rom_x=cx*16 + 0x90, rom_y=cy*16 + 0x90, map_x=cx*16 + 16, map_y=cy*16 + 16)

def rec_out(rec):
    d = {k: rec[k] for k in REC_FIELDS}
    d['flags'] = '0x%04X' % rec['flags']; d['addr'] = '0x%X' % rec['addr']
    if rec['gold'] & 0x8000: d['drop'] = 'item type %d' % (rec['gold'] & 0xF)
    elif rec['gold']: d['drop'] = '%d gold' % rec['gold']
    return d

def main():
    os.makedirs(OUT + '/sheets', exist_ok=True)
    ops = opcode_table()
    json.dump(ops, open(OUT + '/opcodes.json', 'w'), indent=1)
    rooms = []
    # (type table, type, tile_ofs) -> info
    uses = {}
    for room in range(NROOMS):
        tt = type_table(room); at = anim_table(room)
        recs = records(room); precs = records(room, True)
        ent = dict(room=room, type_table='0x%X' % tt, group=group_name(tt), anim_table='0x%X' % at,
                   record_table='0x%X' % L(T_RECORDS + 4*room), persistent_record_table='0x%X' % L(T_PRECORDS + 4*room),
                   sprite_bank='0x%X' % L(T_BANKS + 4*room), palette2='0x%X' % L(T_PAL2 + 4*room), palette3='0x%X' % L(T_PAL3 + 4*room),
                   markers=[], records=[rec_out(x) for x in recs])
        def note(rec, ctx0='default'):
            for t, ofs in ((rec['type'], rec['tile_ofs']), (rec['death_type'], 0)):
                if t >= type_count(tt): continue
                k = (tt, t, ofs if t == rec['type'] else 0)
                u = uses.setdefault(k, dict(rooms=set(), anim_tables={}, records={}))
                u['rooms'].add(room); u['anim_tables'].setdefault((at, ctx0), room)
                if t == rec['type']: u['records']['0x%X' % rec['addr']] = rec_out(rec)
            if rec['gold'] & 0x8000:
                if (rec['gold'] & 0xF) >= type_count(tt): return
                k = (tt, rec['gold'] & 0xF, 0); u = uses.setdefault(k, dict(rooms=set(), anim_tables={}, records={}))
                u['rooms'].add(room); u['anim_tables'].setdefault((at, ctx0), room)
        seeds = []
        k = (tt, 2, 0); u = uses.setdefault(k, dict(rooms=set(), anim_tables={}, records={})); u['rooms'].add(room); u['anim_tables'].setdefault((at, 'default'), room)
        for m in markers(room):
            lst = precs if m['persistent'] else recs
            rec = lst[m['index']] if m['index'] < len(lst) else None
            e = dict(cell=[m['cx'], m['cy']], **world_px(m['cx'], m['cy']), id='0x%02X' % m['id'], table='persistent' if m['persistent'] else 'normal',
                     slot_pool='7-13' if m['high_pool'] else '0-6', index=m['index'], source='0x1C6D6')
            if rec: e['type'] = rec['type']; e['name'] = type_name(tt, rec['type']); e['record'] = '0x%X' % rec['addr']
            if rec: seeds.append(rec)
            ent['markers'].append(e)
        for m in pmarkers(room):
            lst = precs if m['persistent'] else recs
            rec = lst[m['index']] if m['index'] < len(lst) else None
            e = dict(cell=[m['cx'], m['cy']], **world_px(m['cx'], m['cy']), id='0x%02X' % m['id'], table='persistent' if m['persistent'] else 'normal',
                     slot_pool='7-13' if m['high_pool'] else '0-6', index=m['index'], guard_flag=m['guard_flag'], source='0x1CAFA')
            if rec:
                e['type'] = rec['type']; e['name'] = type_name(tt, rec['type']); e['record'] = rec_out(rec)
                seeds.append(rec)
            ent['markers'].append(e)
        # closure: records placed by markers -> their types -> records spawned by those types' opcodes
        # (SPAWN_OPS, normal record table) -> death types / dropped items ...
        seen_r = set(); seen_t = set(); todo = [(x, 'default') for x in seeds]
        while todo:
            rec, ctx0 = todo.pop()
            if (rec['addr'], ctx0) in seen_r: continue
            seen_r.add((rec['addr'], ctx0)); note(rec, ctx0)
            for t in (rec['type'], rec['death_type']):
                if t >= type_count(tt) or (t, ctx0) in seen_t: continue
                seen_t.add((t, ctx0))
                sts = type_states(L(tt + 4*t)); cx = state_contexts(sts, room, ctx0)
                for s_, sc in sts.items():
                    for op in ('I%d' % sc['init'], 'B%d' % sc['behaviour']):
                        for n in spawn_ops(op, room):
                            if n < len(recs): todo.append((recs[n], cx[s_]))
        seen_r = set(a for a, _ in seen_r)
        ent['reachable_records'] = ['0x%X' % a for a in sorted(seen_r)]
        rooms.append(ent)
    json.dump(rooms, open(OUT + '/rooms.json', 'w'), indent=1)

    # every type of every type table (unreferenced ones rendered with the group's first room)
    for room in range(NROOMS):
        tt = type_table(room)
        for t in range(type_count(tt)):
            u = uses.setdefault((tt, t, 0), dict(rooms=set(), anim_tables={}, records={}))
            if not u['anim_tables']: u['anim_tables'][(anim_table(room), 'default')] = room
    index = []
    for (tt, t, ofs), u in sorted(uses.items()):
        sp = L(tt + 4*t)
        if not (0x1EEC2 <= sp < 0x21EEE): continue
        states = type_states(sp)
        if not states: continue
        name = type_name(tt, t)
        d = '%s/%s/t%02d_%s%s' % (OUT, group_name(tt), t, name, '_o%d' % ofs if ofs else '')
        os.makedirs(d, exist_ok=True)
        tj = dict(type=t, name=name, group=group_name(tt), type_table='0x%X' % tt, states_table='0x%X' % sp,
                  tile_ofs=ofs, rooms=sorted(u['rooms']), records=list(u['records'].values()), states={}, animations={})
        for s, sc in states.items():
            tj['states'][s] = dict(addr='0x%X' % sc['addr'], anim=None if sc['anim'] == 0xFF else sc['anim'],
                                   behaviour='B%d' % sc['behaviour'], behaviour_desc=ops.get('B%d' % sc['behaviour'], {}).get('desc'),
                                   init='I%d' % sc['init'], init_desc=ops.get('I%d' % sc['init'], {}).get('desc'),
                                   transitions=[dict(cond='C%d' % c, desc=ops.get('C%d' % c, {}).get('desc'), goto=n) for c, n in sc['conds']])
        frames_all = []
        multi = len(set(a for a, _ in u['anim_tables'])) > 1
        for (at, ctx0), room in sorted(u['anim_tables'].items()):
            ctxs = state_contexts(states, room, ctx0)
            tj['state_gfx'] = {s_: c for s_, c in ctxs.items()} if any(c != 'default' for c in ctxs.values()) else None
            nanim = anim_count(at)
            for a, ctx in sorted(set((sc['anim'], ctxs[s_]) for s_, sc in states.items() if sc['anim'] != 0xFF)):
                if a >= nanim: continue
                vram, pal, palsrc = gfx_for(room, ctx)
                ad = anim_desc(L(at + 4*a))
                suffix = ('@%x' % at if multi else '') + ('' if ctx == 'default' else '#' + ctx)
                if str(a) + suffix in tj['animations']: continue
                key = str(a) + suffix
                aj = dict(anim_table='0x%X' % at, desc_addr='0x%X' % ad['addr'], coll_w=ad['coll_w'], coll_h=ad['coll_h'],
                          last=ad['last'], loop=ad['loop'], rendered_with_room=room, gfx=ctx,
                          palette={k: '0x%X' % v for k, v in palsrc.items()},
                          tile_ofs_applied=ofs if a != 0 else 0, frames=[])
                for k, f in enumerate(ad['frames']):
                    m = mapping(f['mapping'])
                    # tile_ofs (+140) is only added when the anim id (+101) is non-zero (0x14A70)
                    img, ox, oy, lines = render_mapping(f['mapping'], vram, pal, ofs if a != 0 else 0)
                    fn = None
                    if img is not None:
                        fn = 'a%d_f%d%s.png' % (a, k, suffix.replace('@', '_').replace('#', '_'))
                        img.save(d + '/' + fn); frames_all.append((img, ox, oy))
                    aj['frames'].append(dict(png=fn, duration=f['duration'] if f['duration'] < 0xFFF0 else None, raw_duration=f['duration'],
                                             mapping='0x%X' % f['mapping'], origin=[ox, oy], palette_lines=lines,
                                             hurtbox=m['hurt'], attackbox=m['attack'], pieces=len(m['pieces'])))
                tj['animations'][key] = aj
        json.dump(tj, open(d + '/type.json', 'w'), indent=1)
        index.append(dict(dir=os.path.relpath(d, OUT), type=t, name=name, group=group_name(tt), rooms=sorted(u['rooms'])))
        # contact sheet
        if frames_all:
            W_ = sum(i.width for i, _, _ in frames_all) + 2*len(frames_all); H_ = max(i.height for i, _, _ in frames_all)
            if W_ < 8000:
                sh = Image.new('RGBA', (W_, H_), (40, 40, 60, 255)); x = 0
                for i, _, _ in frames_all: sh.alpha_composite(i, (x, H_ - i.height)); x += i.width + 2
                sh.save('%s/sheets/%s_t%02d_%s%s.png' % (OUT, group_name(tt), t, name, '_o%d' % ofs if ofs else ''))
    json.dump(index, open(OUT + '/index.json', 'w'), indent=1)
    print('types', len(index))

if __name__ == '__main__':
    main()
