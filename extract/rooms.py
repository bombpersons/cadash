"""Cadash room extractor.

For every room 0..47 writes into extract/out/rooms/:
  room_NN_fg.png     full foreground layer (plane A), W*16 x H*16, RGBA, colour index 0 = transparent
  room_NN_fg_hi.png  only the 8x8 tiles that have the VDP priority bit set (draw above sprites)
  room_NN_bg.png     plane B (64x32 tiles = 512x256), RGBA, colour index 0 = transparent
                     (the VDP backdrop colour is in the json as `backdrop`; fill behind both layers with it)
  room_NN.json       metadata (see docs/re/world.md for the meaning of every field)
plus rooms_index.json (summary + connectivity graph).

ROM is an asset source only. All addresses documented in docs/re/world.md.
"""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from md import be16, be32, cram_rgb
from lzss import rom, decompress
from PIL import Image

R = rom()
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'out', 'rooms')
NROOMS = 48

# ---- ROM tables -------------------------------------------------------------
T_MAP = 0x52BC0        # room*12: {l metatiles, w len}, {l map, w len}
T_TILES = 0x3BD62      # room*14: {l tilesA, w len}, w offsetB, {l tilesB, w len}
T_ATTR = 0x63318       # room*6 : {l, w} -> 512 bytes, attribute byte per metatile
T_BG = 0x6026C         # room*6 : {l, w} -> plane B nametable 64x32 words
T_PAL2 = 0x4A0A        # room*4 : ptr -> 16 words (palette line 2; also backdrop = line2[0])
T_PAL3 = 0x4C8A        # room*4 : ptr -> 16 words (palette line 3)
T_PALCYC = 0x30B04     # room*4 : ptr -> {w n; n*(w cram byte offset, l seq ptr)}
T_EVENTS = 0x1C6D6     # room*4 : ptr -> (x,y,flags)* 0xFF   enemy/object spawn markers
T_SPAWN_A = 0x1CCAC    # room*4 : ptr -> id*4 : ptr object def  (markers with flags bit6 clear)
T_SPAWN_B = 0x1CD70    # room*4 : ptr -> id*4 : ptr object def  (markers with flags bit6 set)
T_EXIT_DOOR = 0x1388A  # attr 0x2N : room*4 -> ptr -> N*14 exit record   (rooms 0..28 only)
T_EXIT_EDGE = 0x13572  # attr 0x4N : room*4 -> ptr -> N*14 exit record   (rooms 0..43 only)
T_EXIT_WARP = 0x13B82  # attr 0x5N : N*14 exit record (global table)
T_EXIT_EVENT = 0x13BD6  # FF0A74=k (set by boss objects) : (k-1)*14 exit record
T_STAIRS = 0x4992      # attr 0x3N : N*6 {w dy, w lock timer (FF0B56), w player+0x84}
T_MUSIC = 0x49DA       # room byte: BGM id played on room entry (via FF0A56 delay)
T_STAGE_START = 0x47A0  # stage (FF0A4C)*6 : {l table, w index} start point used by stage select

N_DOOR_ROOMS = (0x138FE - 0x1388A) // 4   # 29
N_EDGE_ROOMS = (0x13622 - 0x13572) // 4   # 44


def s16(v):
    return v - 0x10000 if v & 0x8000 else v


def exit_record(p):
    room, camx, camy, x, y, face, music = [be16(R, p + 2 * j) for j in range(7)]
    return {
        'dest_room': room,
        'dest_camera_blocks': [camx, camy],          # FF1EDA/FF1EDC (16px blocks), camera top-left
        'dest_screen_raw': [x, y],                    # player x/y written relative to camera, +128 VDP bias
        'dest_world_px': [camx * 16 + x - 128, camy * 16 + y - 128],  # player foot point in room pixels
        'dest_facing': 'right' if face else 'left',  # player+0x76: 1 = facing right
        'music': music,                               # 0 = keep current, else sound id passed to 0x2642
    }


def load_room(i):
    b = T_MAP + i * 12
    mt = decompress(R, be32(R, b), be16(R, b + 4))
    m = decompress(R, be32(R, b + 6), be16(R, b + 10))
    W = m[0] * m[2]; H = m[1] * m[3]
    cells = [be16(m, 4 + 2 * k) & 0x1FF for k in range(W * H)]
    a = decompress(R, be32(R, T_ATTR + i * 6), be16(R, T_ATTR + i * 6 + 4))
    # tiles: buffer = A at 0, B at offset; then buf[0x2000:0x3000] = buf[0x3000:0x4000];
    # 0x3000 bytes DMA'd to VRAM 0x2000 -> tiles 0x100..0x27F
    t = T_TILES + i * 14
    A = decompress(R, be32(R, t), be16(R, t + 4))
    off = be16(R, t + 6)
    B = decompress(R, be32(R, t + 8), be16(R, t + 12))
    buf = bytearray(0x5000)
    buf[0:len(A)] = A
    buf[off:off + len(B)] = B
    buf[0x2000:0x3000] = buf[0x3000:0x4000]
    vram = bytearray(0x10000)
    vram[0x2000:0x5000] = buf[0:0x3000]
    pal = [[0] * 16 for _ in range(4)]
    pal[2] = [be16(R, be32(R, T_PAL2 + 4 * i) + 2 * k) for k in range(16)]
    pal[3] = [be16(R, be32(R, T_PAL3 + 4 * i) + 2 * k) for k in range(16)]
    bgp = be32(R, T_BG + i * 6)
    bg = decompress(R, bgp, be16(R, T_BG + i * 6 + 4))[:4096]
    return dict(W=W, H=H, hdr=list(m[:4]), mt=mt, cells=cells, attr=a, vram=vram, pal=pal, bg=bg,
                ptrs=dict(metatiles=be32(R, b), map=be32(R, b + 6), tilesA=be32(R, t), tilesB=be32(R, t + 8),
                          tilesB_offset=off, attr=be32(R, T_ATTR + i * 6), bg=bgp))


def draw_tile(px, vram, pal, word, ox, oy, only_hi=False):
    if only_hi and not (word & 0x8000):
        return
    t = word & 0x7FF
    if t < 0x100:          # tile 0 / common area: blank in all rooms (only tile 0 is referenced)
        return
    line = (word >> 13) & 3
    hf = word & 0x800; vf = word & 0x1000
    base = t * 32
    for y in range(8):
        sy = 7 - y if vf else y
        for x in range(8):
            sx = 7 - x if hf else x
            b = vram[base + sy * 4 + (sx >> 1)]
            c = (b >> 4) if (sx & 1) == 0 else (b & 15)
            if c:
                px[ox + x, oy + y] = cram_rgb(pal[line][c]) + (255,)


def render_fg(rm, only_hi=False):
    W, H = rm['W'], rm['H']
    img = Image.new('RGBA', (W * 16, H * 16), (0, 0, 0, 0)); px = img.load()
    mt = rm['mt']
    for cy in range(H):
        for cx in range(W):
            c = rm['cells'][cy * W + cx]
            for q in range(4):
                w = be16(mt, c * 8 + 2 * q)
                draw_tile(px, rm['vram'], rm['pal'], w, cx * 16 + (q & 1) * 8, cy * 16 + (q >> 1) * 8, only_hi)
    return img


def render_bg(rm):
    img = Image.new('RGBA', (512, 256), (0, 0, 0, 0)); px = img.load()
    for ty in range(32):
        for tx in range(64):
            draw_tile(px, rm['vram'], rm['pal'], be16(rm['bg'], (ty * 64 + tx) * 2), tx * 8, ty * 8)
    return img


def components(W, H, mask):
    """4-connected components of cells where mask(x,y) true -> list of cell lists."""
    seen = set(); comps = []
    for y in range(H):
        for x in range(W):
            if (x, y) in seen or not mask(x, y):
                continue
            st = [(x, y)]; seen.add((x, y)); comp = []
            while st:
                cx, cy = st.pop(); comp.append((cx, cy))
                for nx, ny in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                    if 0 <= nx < W and 0 <= ny < H and (nx, ny) not in seen and mask(nx, ny):
                        seen.add((nx, ny)); st.append((nx, ny))
            comps.append(comp)
    return comps


def exits_for(i, rm):
    W, H = rm['W'], rm['H']
    at = [rm['attr'][c] for c in rm['cells']]
    vals = sorted(set(v for v in at if 2 <= (v >> 4) <= 5))
    out = []
    for v in vals:
        kind = v >> 4; n = v & 15
        for comp in components(W, H, lambda x, y: at[y * W + x] == v):
            xs = [c[0] for c in comp]; ys = [c[1] for c in comp]
            e = {'attr': v, 'index': n,
                 'trigger_blocks': [min(xs), min(ys), max(xs) + 1, max(ys) + 1],   # x0,y0,x1,y1 (exclusive)
                 'trigger_px': [min(xs) * 16, min(ys) * 16, (max(xs) + 1) * 16, (max(ys) + 1) * 16],
                 'cells': len(comp)}
            if kind == 2:
                e['kind'] = 'door'; e['needs_up'] = True
                p = be32(R, T_EXIT_DOOR + 4 * i) if i < N_DOOR_ROOMS else 0
                e.update(exit_record(p + 14 * n) if p else {'dest_room': None, 'note': 'no door table for this room'})
                if i == 12:
                    e['condition'] = 'player+0x74 (swimming flag) must be set'
            elif kind == 3:
                e['kind'] = 'stairs'; e['needs_up'] = True
                q = T_STAIRS + 6 * n
                e.update({'dest_room': i, 'dy_px': s16(be16(R, q)), 'lock_frames': be16(R, q + 2),
                          'player_0x84': be16(R, q + 4)})
            elif kind == 4:
                e['kind'] = 'edge'; e['needs_up'] = False
                p = be32(R, T_EXIT_EDGE + 4 * i) if i < N_EDGE_ROOMS else 0
                if i == 45 or not p:
                    e.update({'dest_room': None, 'note': 'disabled (room 45 is excluded in code / no table)'})
                else:
                    e.update(exit_record(p + 14 * n))
            elif kind == 5:
                e['kind'] = 'warp'; e['needs_up'] = True; e['sound'] = 0xB7
                e.update(exit_record(T_EXIT_WARP + 14 * n))
            e['_cells'] = comp
            out.append(e)
    # a component lying entirely on top of another exit (e.g. the top metatile row of a doorway with a
    # different attribute) can never be the player's (x, foot-1) cell while standing -> mark it shadowed
    allc = {}
    for k, e in enumerate(out):
        for c in e['_cells']:
            allc[c] = k
    for k, e in enumerate(out):
        e['shadowed'] = all(allc.get((x, y + 1), k) != k for x, y in e['_cells'])
    for e in out:
        del e['_cells']
    return out


def events_for(i):
    p = be32(R, T_EVENTS + 4 * i); ev = []
    while R[p] != 0xFF:
        x, y, f = R[p], R[p + 1], R[p + 2]; p += 3
        if f & 0x80:
            kind, sid, tbl = 'enemy_pool_hi', f & 0x3F, T_SPAWN_A      # cell |= 0x4000, slots from 0xFF17C0
        elif f & 0x40:
            kind, sid, tbl = 'persistent', f & 0x3F, T_SPAWN_B         # cell |= 0x8000, one-shot flag in 0xFF0D40
        else:
            kind, sid, tbl = 'enemy', f, T_SPAWN_A                     # slots 0xFF10C0.. (first 7)
        sid &= 0xF   # only bits 9..12 survive in the cell word (bit 13 is the 'spawned' flag)
        rp = be32(R, tbl + 4 * i)
        dp = be32(R, rp + 4 * sid) if rp else 0
        ev.append({'block': [x, y], 'flags': f, 'kind': kind, 'spawn_id': sid,
                   'spawn_world_px': [x * 16 + 16, y * 16 + 16],
                   'def_ptr': hex(dp) if dp else None})
    return ev


def palcycle_for(i):
    p = be32(R, T_PALCYC + 4 * i); n = be16(R, p); out = []
    for k in range(n):
        off = be16(R, p + 2 + 6 * k); sp = be32(R, p + 4 + 6 * k); seq = []; q = sp
        while be16(R, q) != 0xFFFF:
            d, c = be16(R, q), be16(R, q + 2)
            seq.append({'frames': d + 1, 'md': '%03X' % c, 'rgb': list(cram_rgb(c))}); q += 4
        out.append({'line': off // 32, 'index': (off % 32) // 2, 'steps': seq})
    return out


def priority_info(rm):
    mt = rm['mt']; used = sorted(set(rm['cells'])); hi = {}
    for c in used:
        qs = [q for q in range(4) if be16(mt, c * 8 + 2 * q) & 0x8000]
        if qs:
            hi[c] = qs
    lines = sorted(set((be16(mt, c * 8 + 2 * q) >> 13) & 3 for c in used for q in range(4)))
    return hi, lines


ROOM_DESC = {}


def main():
    os.makedirs(OUT, exist_ok=True)
    music = list(R[T_MUSIC:T_MUSIC + NROOMS])
    index = []
    for i in range(NROOMS):
        rm = load_room(i)
        W, H = rm['W'], rm['H']
        render_fg(rm).save(os.path.join(OUT, 'room_%02d_fg.png' % i))
        hi, lines = priority_info(rm)
        render_fg(rm, only_hi=True).save(os.path.join(OUT, 'room_%02d_fg_hi.png' % i))
        render_bg(rm).save(os.path.join(OUT, 'room_%02d_bg.png' % i))
        exits = exits_for(i, rm)
        bg_lines = sorted(set((be16(rm['bg'], 2 * k) >> 13) & 3 for k in range(2048) if be16(rm['bg'], 2 * k) & 0x7FF))
        if i == 11:
            bgscroll = {'x': 0.0, 'y': 0.0, 'note': 'room 11: plane B scroll update skipped (static)'}
        else:
            bgscroll = {'x': 0.5, 'y': 0.5,
                        'note': 'B offset = (camera - camera_at_room_entry) / 2 (x: hscroll -dx/2, y: vscroll +dy/2), '
                                'B starts at offset 0,0 on room entry; wraps at 512x256'}
        j = {
            'room': i,
            'width_blocks': W, 'height_blocks': H, 'block_px': 16,
            'screens': [rm['hdr'][0], rm['hdr'][1]], 'screen_blocks': [rm['hdr'][2], rm['hdr'][3]],
            'cells': rm['cells'],
            'attrs': [rm['attr'][c] for c in rm['cells']],
            'palette_line2': [list(cram_rgb(c)) for c in rm['pal'][2]],
            'palette_line3': [list(cram_rgb(c)) for c in rm['pal'][3]],
            'palette_line2_md': ['%03X' % c for c in rm['pal'][2]],
            'palette_line3_md': ['%03X' % c for c in rm['pal'][3]],
            'backdrop': list(cram_rgb(rm['pal'][2][0])),
            'fg_palette_lines': lines, 'bg_palette_lines': bg_lines,
            'bg_png': 'room_%02d_bg.png' % i, 'bg_transparent_index0': True,
            'bg_scroll': bgscroll,
            'priority_metatiles': {str(k): v for k, v in hi.items()},  # metatile -> quadrants (0 TL,1 TR,2 BL,3 BR)
            'palette_cycles': palcycle_for(i),
            'music': music[i],
            'exits': exits,
            'events': events_for(i),
            'rom': {k: hex(v) for k, v in rm['ptrs'].items()},
        }
        with open(os.path.join(OUT, 'room_%02d.json' % i), 'w') as f:
            json.dump(j, f, separators=(',', ':'))
        index.append({'room': i, 'w': W, 'h': H, 'music': music[i],
                      'exits': [{k: e.get(k) for k in ('kind', 'attr', 'trigger_blocks', 'shadowed', 'dest_room',
                                                         'dest_world_px', 'dest_facing', 'dy_px')} for e in exits]})
    # global start points
    starts = {'new_game': {'room': 0, 'camera_blocks': [0x36, 0xF], 'player_screen_raw': [0x11C, 0x100],
                           'player_world_px': [0x36 * 16 + 0x11C - 128, 0xF * 16 + 0x100 - 128], 'facing': 'right',
                           'note': 'set at 0x45AC (camera) and 0x907C (player); 2-player P1 x=0x130'},
              'stage_select': [], 'event_warps': []}
    for s in range(5):
        tbl = be32(R, T_STAGE_START + 6 * s); idx = be16(R, T_STAGE_START + 6 * s + 4)
        if tbl == 0:
            starts['stage_select'].append({'stage': s, 'room': 0, 'note': 'new game start'})
        elif tbl == T_EXIT_WARP:
            starts['stage_select'].append(dict(stage=s, **exit_record(tbl + 14 * idx)))
        else:   # per-room table indexed by preset room (stage room table 0x6FEE)
            room = be16(R, 0x6FEE + 2 * s)
            starts['stage_select'].append(dict(stage=s, **exit_record(be32(R, tbl + 4 * room) + 14 * idx)))
    for k in range(1, 5):
        starts['event_warps'].append(dict(ff0a74=k, **exit_record(T_EXIT_EVENT + 14 * (k - 1))))
    with open(os.path.join(OUT, 'rooms_index.json'), 'w') as f:
        json.dump({'rooms': index, 'starts': starts}, f, indent=1)
    print('wrote', OUT)


if __name__ == '__main__':
    main()
