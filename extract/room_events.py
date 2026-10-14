"""Per-room scripted events (cartridge dispatcher 0x31082, room-entry hook 0x30EC6, vblank hook 0x3183A).

Writes into game/assets/:
  rooms/room_02_bg_upper.png, rooms/room_02_bg_lower.png   alternate plane-B nametables of room 2
  rooms/room_02_bg_<variant>_cycle_<line>_<idx>.png        white masks for the variant's palette cycles
  rooms/room_13_silk.png                                   the Silk of the Worm thread (mapping 0x319F0)
  rooms/room_19_rain.png                                   the rain band (mapping 0x31A2A, screen space)
  rooms/room_19_fg_closed.png                              room 19 foreground before the flame bridge exists
  rooms/room_19_bridge_block.png                           one bridge block (metatile 0x0D)
  data/room_events.events.json                             all parameters (see docs/re/room_events.md)

Usage: python3 extract/room_events.py   (needs only the ROM). build_assets.py rooms() calls this too,
because it empties game/assets/rooms/ first.
"""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from md import be16, be32, cram_rgb, tile_pixels
from lzss import rom, decompress
from PIL import Image
import rooms as RM

R = rom()
HERE = os.path.dirname(os.path.abspath(__file__))
GAME = os.path.join(HERE, '..', 'game', 'assets')
ROOMS = os.path.join(GAME, 'rooms')

# Plane-B nametables loaded by the room-2 handler (0x310FA) with their LZSS lengths.
ROOM2_BG = {
    'default': (0x60688, 0x240),
    'upper': (0x62B9C, 0x27C),
    'lower': (0x62E18, 0x500),
}
# Palette-cycle tables (room*4 -> {w n; n*(w cram offset, l seq)}), all read with the room index.
CYCLE_TABLES = {'default': 0x30B04, 'upper': 0x30BC4, 'lower': 0x30BD0}


def s16(v):
    return v - 0x10000 if v & 0x8000 else v


def cycles_from(table, room):
    p = be32(R, table + 4 * room); n = be16(R, p); out = []
    for k in range(n):
        off = be16(R, p + 2 + 6 * k); q = be32(R, p + 4 + 6 * k); seq = []
        while be16(R, q) != 0xFFFF:
            seq.append({'frames': be16(R, q) + 1, 'rgb': list(cram_rgb(be16(R, q + 2)))}); q += 4
        out.append({'line': off // 32, 'index': (off % 32) // 2, 'steps': seq})
    return out


def mask_tile(px, vram, word, line, idx, ox, oy):
    t = word & 0x7FF
    if t < 0x100 or ((word >> 13) & 3) != line:
        return False
    hf = word & 0x800; vf = word & 0x1000; base = t * 32; hit = False
    for y in range(8):
        sy = 7 - y if vf else y
        for x in range(8):
            sx = 7 - x if hf else x
            b = vram[base + sy * 4 + (sx >> 1)]
            c = (b >> 4) if (sx & 1) == 0 else (b & 15)
            if c == idx:
                px[ox + x, oy + y] = (255, 255, 255, 255); hit = True
    return hit


def render_nametable(rm, nt):
    img = Image.new('RGBA', (512, 256), (0, 0, 0, 0)); px = img.load()
    for ty in range(32):
        for tx in range(64):
            RM.draw_tile(px, rm['vram'], rm['pal'], be16(nt, (ty * 64 + tx) * 2), tx * 8, ty * 8)
    return img


def room2():
    rm = RM.load_room(2)
    variants = {}
    for name, (ptr, clen) in ROOM2_BG.items():
        nt = decompress(R, ptr, clen)[:4096]
        image = 'rooms/room_02_bg.png'
        if name != 'default':
            image = f'rooms/room_02_bg_{name}.png'
            render_nametable(rm, nt).save(os.path.join(GAME, image))
        cycles = []
        for c in cycles_from(CYCLE_TABLES[name], 2):
            masks = {}
            img = Image.new('RGBA', (512, 256), (0, 0, 0, 0)); px = img.load(); hit = False
            for ty in range(32):
                for tx in range(64):
                    hit |= mask_tile(px, rm['vram'], be16(nt, (ty * 64 + tx) * 2), c['line'], c['index'], tx * 8, ty * 8)
            if hit:
                path = f'rooms/room_02_bg_{name}_cycle_{c["line"]}_{c["index"]}.png'
                img.save(os.path.join(GAME, path)); masks['bg'] = path
            # The room-2 foreground never uses the cycled indices (line 3, colours 10..13), so only bg masks exist.
            if masks:
                cycles.append({'masks': masks, 'steps': c['steps']})
        variants[name] = {'image': image, 'rom_nametable': hex(ptr), 'rom_cycles': hex(CYCLE_TABLES[name])}
        variants[name]['cycles'] = cycles
    # VDP scroll values written on a swap: hscroll 0xFF0642, vscroll 0xFF05F2; freeze = 0xFF0B99 bits 0 (x), 1 (y).
    variants['default'].update(hscroll=0, vscroll=0, freeze=[False, False])
    variants['upper'].update(hscroll=0x1A0, vscroll=0, freeze=[True, True])
    variants['lower'].update(hscroll=0, vscroll=0x40, freeze=[False, True])
    # Checked whenever the camera block x (0xFF1EDA) changes; camera block y (0xFF1EDC) < 10 = upper half.
    triggers = [
        {'cam_block_y': [0, 10], 'cam_block_x': [0x5A, 0x1B], 'variant': 'default', 'from': 'other'},
        {'cam_block_y': [0, 10], 'cam_block_x': [0x59, 0x1C], 'variant': 'upper', 'from': 'default'},
        {'cam_block_y': [10, 9999], 'cam_block_x': [0x8C, 0x3B], 'variant': 'default', 'from': 'other'},
        {'cam_block_y': [10, 9999], 'cam_block_x': [0x8B, 0x3C], 'variant': 'lower', 'from': 'default'},
    ]
    shake = {'amplitude': 3, 'cam_block_y_min': 10, 'cam_block_x': [0x45, 0x86],
             'note': 'odd frames: planes A and B vscroll += s, s = -s (s starts at +3 on room entry, 0x30F38)'}
    return {'room': 2, 'variants': variants, 'triggers': triggers, 'shake': shake}


def render_pieces(pieces, vram, pal):
    """pieces = [(dx, dy, size, attr)] -> (image, origin x, origin y). First piece on top."""
    boxes = [(dx, dy, dx + 8 * (((s >> 2) & 3) + 1), dy + 8 * ((s & 3) + 1)) for dx, dy, s, a in pieces]
    x0 = min(b[0] for b in boxes); y0 = min(b[1] for b in boxes)
    x1 = max(b[2] for b in boxes); y1 = max(b[3] for b in boxes)
    img = Image.new('RGBA', (x1 - x0, y1 - y0), (0, 0, 0, 0)); px = img.load()
    for dx, dy, size, attr in reversed(pieces):
        w = ((size >> 2) & 3) + 1; h = (size & 3) + 1
        t0 = attr & 0x7FF; line = (attr >> 13) & 3; hf = attr & 0x800; vf = attr & 0x1000
        for cx in range(w):
            for cy in range(h):
                tile = tile_pixels(vram, (t0 + cx * h + cy) & 0x7FF)
                tx = (w - 1 - cx) if hf else cx; ty = (h - 1 - cy) if vf else cy
                for y in range(8):
                    for x in range(8):
                        c = tile[7 - y if vf else y][7 - x if hf else x]
                        if c:
                            px[dx - x0 + tx * 8 + x, dy - y0 + ty * 8 + y] = pal[line * 16 + c] + (255,)
    return img, -x0, -y0


def sprite_vram_pal(room):
    import objects as O
    pal, _ = O.room_palette(room)
    return O.room_vram(room), pal


def room13():
    # Silk thread object (0xFF1DC0) drawn by 0x2794 with the mapping at 0x319F0 (w count, 16-byte header, pieces).
    mp = 0x319F0; n = be16(R, mp)
    pieces = [(s16(be16(R, mp + 18 + 8 * k)), s16(be16(R, mp + 20 + 8 * k)), be16(R, mp + 22 + 8 * k) & 0xFF,
               be16(R, mp + 24 + 8 * k)) for k in range(n)]
    vram, pal = sprite_vram_pal(13)
    img, ox, oy = render_pieces(pieces, vram, pal)
    img.save(os.path.join(ROOMS, 'room_13_silk.png'))
    rm = RM.load_room(13); W = rm['W']
    # Metatiles 0xF0..0xF3 get attribute 1 (rope) once the thread is down: write 0x01010101 to 0xFF6C94.
    cells = [[i % W, i // W] for i, c in enumerate(rm['cells']) if 0xF0 <= c <= 0xF3]
    return {
        'room': 13, 'cam_block_x': 0xC, 'cam_block_y': [0x15, 0x20], 'item_flag': '0xFF0ACE',
        'message_ok': 0x53, 'message_ok_shape': 1, 'message_fail': 0x5C, 'message_fail_shape': 1,
        'sprite': {'image': 'rooms/room_13_silk.png', 'origin': [ox, oy]},
        # raw VDP coordinates (0x194, 0x1A0) .. y 0x220, step 2 per frame; world = raw - 128
        'start': [0x194 - 128, 0x1A0 - 128], 'end_y': 0x220 - 128, 'step': 2,
        'rope_cells': cells, 'rope_attr': 1,
    }


def room19():
    vram, pal = sprite_vram_pal(19)
    # Rain band (0xFFF414, mapping 0x31A2A without a header, drawn in screen space by 0x283E in vblank 0x3183A).
    mp = 0x31A2A; n = be16(R, mp)
    pieces = [(s16(be16(R, mp + 2 + 8 * k)), s16(be16(R, mp + 4 + 8 * k)), be16(R, mp + 6 + 8 * k) & 0xFF,
               be16(R, mp + 8 + 8 * k)) for k in range(n)]
    img, ox, oy = render_pieces(pieces, vram, pal)
    img.save(os.path.join(ROOMS, 'room_19_rain.png'))
    rm = RM.load_room(19); W = rm['W']
    # Flame bridge: map cells at 0xFF3F28.. = row 9, columns 186..199 (14 cells).
    row, c0, ncells = 9, (0xFF3F28 - 0xFF2FA4) // 2 % W, 14
    assert (0xFF3F28 - 0xFF2FA4) // 2 // W == row
    bridge_mt = 0x0D
    closed = dict(rm); closed['cells'] = list(rm['cells'])
    for k in range(ncells):
        closed['cells'][row * W + c0 + k] = 0          # entry with 0xFF0AE6 clear writes 0x2000 (metatile 0)
    RM.render_fg(closed).save(os.path.join(ROOMS, 'room_19_fg_closed.png'))
    blk = Image.new('RGBA', (16, 16), (0, 0, 0, 0)); px = blk.load()
    for q in range(4):
        RM.draw_tile(px, rm['vram'], rm['pal'], be16(rm['mt'], bridge_mt * 8 + 2 * q), (q & 1) * 8, (q >> 1) * 8)
    blk.save(os.path.join(ROOMS, 'room_19_bridge_block.png'))
    wall = [[i % W, i // W] for i, c in enumerate(rm['cells']) if c == 0xFC]   # metatile 0xFC attr -> 0 (0xFF6CA0)
    return {
        'room': 19,
        'bridge': {
            'done_flag': '0xFF0AE6', 'item_flag': '0xFF0AD8',
            'cam_block_x': 0xAF, 'message_ok': 0x55, 'message_ok_shape': 1, 'message_fail': 0x5E, 'message_fail_shape': 1,
            'sound': 0x93, 'wait_frames': 0xFF, 'block_every': 4, 'blocks_drawn': 10,
            'row': row, 'col0': c0, 'cells': ncells, 'open_attr': rm['attr'][bridge_mt], 'closed_attr': rm['attr'][0],
            'wall_cells': wall, 'block_image': 'rooms/room_19_bridge_block.png',
            'fg_closed': 'rooms/room_19_fg_closed.png',
        },
        'rain': {
            'image': 'rooms/room_19_rain.png', 'origin': [ox, oy],
            # screen coordinates (raw - 128): x 0, y 0x70..0xF0 step 0x20 per vblank
            'x': 0, 'y0': 0x70 - 128, 'y_end': 0x110 - 128, 'step': 0x20,
            'on_right': 0x13, 'off_right': 0x9F, 'on_left': 0x73, 'off_left': 0x0C,
        },
        'grave': {'attr': 0x70, 'done_flag': '0xFF0AEA', 'item_flag': '0xFF0AD4',
                  'message_read': 0x5D, 'message_read_shape': 1, 'message_charm': 0x39, 'message_charm_shape': 0},
    }


def room20():
    rm = RM.load_room(20); W = rm['W']
    cells = [[i % W, i // W] for i, c in enumerate(rm['cells']) if c in (0x136, 0x137)]
    return {'room': 20, 'flag': '0xFF0AE0', 'done_flag': '0xFF0AE8', 'cells': cells, 'attr': 0,
            'note': '0xFF6CDA = 0: metatiles 0x136/0x137 lose their solid attribute (the wall Alex tells you to push)'}


def demo_stream(a):
    out = []
    while not (R[a] == 0 and R[a + 1] == 0):
        out.append([R[a], R[a + 1]]); a += 2
    return out


def pal16(a):
    return [list(cram_rgb(be16(R, a + 2 * k))) for k in range(16)]


def words16(a):
    return [be16(R, a + 2 * k) for k in range(16)]


def main():
    os.makedirs(ROOMS, exist_ok=True)
    os.makedirs(os.path.join(GAME, 'data'), exist_ok=True)
    out = {
        'note': 'Per-room events of the cartridge (dispatcher 0x31082, entry hook 0x30EC6); see docs/re/room_events.md',
        'bg_swaps': [room2()],
        'silk_rope': room13(),
        'graveyard': room19(),
        'wall_unlock': [room20()],
        'flag_jingles': [
            {'room': 4, 'flag': '0xFF0ADE', 'done_flag': '0xFF0AEE', 'sound': 0x90, 'music_timer': 0x127},
            {'room': 27, 'flag': '0xFF0AE4', 'done_flag': '0xFF0AF2', 'sound': 0x90, 'music_timer': 0x127},
        ],
        'forest_loop': {'room': 12, 'cam_block_x': 10, 'shift_blocks': 10},
        'gnome': {'room': 12, 'item_flag': '0xFF0AD0', 'shrink_attr': 0x60, 'grow_attr': 0x61,
                  'message': 0x52, 'message_shape': 1, 'sound': 0xA3},
        # 0xFF0B99 bits: 0 freeze plane-B x, 1 freeze plane-B y, 2 freeze camera x, 3 freeze camera y.
        'entry_freeze': [{'room': 9, 'bits': 2}, {'room': 44, 'bits': 15}, {'room': 46, 'bits': 15}, {'room': 47, 'bits': 15}],
        'entry_messages': [{'room': 9, 'message': 0x5B, 'shape': 1, 'unless_flag': '0xFF0AF0'}],
        'player_priority_rooms': [22],
        # Text in room 0x2F pages/ends on timers (0x3D / 0x51 frames, see docs/re/ui_rpg.md).
        'ending_room': {'room': 47, 'delay': 60, 'message': 0x46, 'shape': 0, 'timed': [0x3D, 0x51], 'demo': demo_stream(0x6336),
                        # entry hook 0x30FEE: +0xA2 walk speed := 1.0 (16.16), +0xA6 poison := 0 (both players)
                        'walk_speed': be32(R, 0x31028 + 2), 'clear_poison': True},
        # Sprite palette line 2 swaps (0x3189A / 0x318CA), applied by the remake's sprite_palette.rs;
        # 'words' = CRAM words, 'palette' = the same as RGB.
        'palette_line2_swaps': [
            {'room': 24, 'flag': '0xFF0AEC', 'palette': pal16(0x4BCA), 'words': words16(0x4BCA), 'rom': '0x4BCA'},
            {'room': 27, 'cam_block_x_max': 12, 'cam_block_y': 13, 'palette': pal16(0x4ACA), 'words': words16(0x4ACA), 'rom': '0x4ACA'},
            {'room': 27, 'cam_block_x_max': 12, 'cam_block_y': 14, 'palette': pal16(0x4BEA), 'words': words16(0x4BEA), 'rom': '0x4BEA'},
        ],
    }
    with open(os.path.join(GAME, 'data', 'room_events.events.json'), 'w') as f:
        json.dump(out, f, indent=1)
    print('room events written')


if __name__ == '__main__':
    main()
