#!/usr/bin/env python3
"""Cadash shop / inn / pause-window UI extraction (spec: docs/re/shops.md).

Outputs
  game/assets/data/shops.ui.json   machine-readable shop + window + equipment-visual data
  game/assets/ui/shop_icons.png      one 16x16 icon per shop record (record order 0..21), index 0 transparent
  game/assets/ui/shop_cursors.png    8x8 cursor arrows: [P1 (tile 0x24D), P2 (tile 0x24F)]
  work/shops/mock_*.png              static mock-ups of every shop screen (layout validation, not game assets)
All numbers come from the ROM; addresses are documented next to each table.
"""
import json, os, struct, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lzss import decompress
from md import cram_rgb
from PIL import Image
import rooms

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
R = open(os.path.join(ROOT, 'Cadash (USA, Korea) (En).md'), 'rb').read()
GAME = os.path.join(ROOT, 'game', 'assets')
WORK = os.path.join(ROOT, 'work', 'shops')
def be16(a): return struct.unpack('>H', R[a:a + 2])[0]
def be32(a): return struct.unpack('>I', R[a:a + 4])[0]
def words(a, n): return [be16(a + 2 * i) for i in range(n)]
def hx(v, n=4): return '0x%0*X' % (n, v)

STR = json.load(open(os.path.join(ROOT, 'extract/out/text/strings.json')))
TEXT = {e['id']: e['text'] for e in STR['dialogue_table']['entries']}

# ---------------------------------------------------------------- window shapes (0x2CEA2)
def window(shape):
    w, h, cx, cy = struct.unpack('>4h', R[0x2CEA2 + 8 * shape:0x2CEA2 + 8 * shape + 8])
    return {'shape': shape, 'w_cells': w, 'h_cells': h,
            'top_left_px': [cx - 128 - w * 4, cy - 128 - h * 4 + 8]}
WINDOWS = {s: window(s) for s in range(15)}

# ---------------------------------------------------------------- shop records (0x33076, 12 bytes)
SLOT = {0xFF0AC0: 'fighter_armor', 0xFF0AC2: 'fighter_helm', 0xFF0AC4: 'fighter_shield', 0xFF0AC6: 'fighter_weapon',
        0xFF0AC8: 'mage_robe', 0xFF0ACA: 'mage_wand'}
BONUS = {'fighter_weapon': (0xC790, 'STR', 5), 'fighter_armor': (0xC796, 'AC', 4), 'fighter_helm': (0xC79A, 'AC', 3),
         'fighter_shield': (0xC79E, 'AC', 4), 'mage_wand': (0xC7A2, 'STR', 4), 'mage_robe': (0xC7A6, 'AC', 3)}
NAMES = ['Medical Herb', 'Antidote', 'Bell of Life', 'Extra Life', 'Silver Key', 'Broadsword', 'Chain Mail',
         'Iron Wand', 'Robe of the Quester', 'Blade of Fire', 'Leather Shield', 'Staff of Fire', 'Sword of Speed',
         'Plate Armor', "Mystic's Robe", 'Silver Shield', 'Silver Helm', 'Staff of the Mage', 'Battle Sword',
         'Golden Armor', 'Golden Shield', 'Golden Helm']
CLASS = {0: 'fighter', 1: 'mage', 2: 'any'}
SPECIAL = {
    'Blade of Fire': 'attacks also launch a fireball (0xB692)',
    'Staff of Fire': 'attacks also launch a fireball (0xB692)',
    'Sword of Speed': 'attack frame duration -1 (0xC75E)',
    'Staff of the Mage': '+1 MP every 420 frames (0xC262)',
    'Leather Shield': 'any shield blocks blockable projectiles from the facing side (0xA0EA)',
    'Silver Shield': 'any shield blocks blockable projectiles from the facing side (0xA0EA)',
    'Golden Shield': 'any shield blocks blockable projectiles from the facing side (0xA0EA)',
}
records = []
for i in range(22):
    b = 0x33076 + 12 * i
    tgt, tid, cls, val, price = struct.unpack('>IHHHH', R[b:b + 12])
    rec = {'record': i, 'addr': hx(b, 5), 'name': NAMES[i], 'class': CLASS[cls], 'text_id': tid,
           'text': TEXT.get(tid, ''), 'icon_index': i}
    if i < 3:
        rec.update(kind='consumable', target={'player_field': hx(tgt, 2)}, max_count=val, price=price,
                   effect='recipient.%s += 1 (refused if already >= %d)' % (hx(tgt, 2), val))
        if i == 2:
            rec['effect'] += '; recipient max HP (+0x8E) += 8, cap 255; HUD redrawn (0x32C24). Current HP unchanged.'
        rec['counter'] = {0: 'herbs', 1: 'antidotes', 2: 'bells'}[i]
    elif i == 3:
        rec.update(kind='extra_life', target={'global': hx(tgt, 6)}, max_count=val, price=None,
                   price_dynamic={'ram': '0xFF0A78', 'initial': be16(0x4572), 'after_purchase': 'x2, cap 6400 (0x1900)',
                                  'record_price_field_unused': price},
                   effect='lives (0xFF0A7A, shared) += 1 (refused at >= 9); price doubles; price digits in the '
                          'list are redrawn (0x32CF6) and the lives sprite refreshed (0x2BEFE)')
    elif i == 4:
        rec.update(kind='key_item', target={'global': hx(tgt, 6)}, max_count=val, price=price,
                   effect='0xFF0AD2 = 1 (silver key; refused if already owned)')
    else:
        slot = SLOT[tgt]
        ta, stat, n = BONUS[slot]
        rec.update(kind='equipment', slot=slot, target={'global': hx(tgt, 6)}, tier=val, price=price,
                   bonus={'stat': stat, 'value': R[ta + val], 'table': hx(ta, 5)},
                   effect='%s = %d (refused if current tier >= %d); stats recomputed (0xC5B0, HP/MP maxima untouched), '
                          'player palette + equipment tiles rebuilt' % (hx(tgt, 6), val, val))
        if NAMES[i] in SPECIAL:
            rec['special'] = SPECIAL[NAMES[i]]
    records.append(rec)

# ---------------------------------------------------------------- shop blocks (selected by room in 0x32574..0x325F6)
ARMORY_BLOCKS = [(30, 0x3317E), (33, 0x33238), (34, 0x332F2), (39, 0x333AC)]
ARMORY_DEFAULT = 0x33466
APO_BLOCKS = [(32, 0x33520), (43, 0x335DA)]
APO_DEFAULT = 0x33694
SHOP_ROOMS = {29: 'inn', 30: 'armory', 32: 'apothecary', 33: 'armory', 34: 'armory', 35: 'inn', 36: 'apothecary',
              39: 'armory', 40: 'inn', 42: 'armory', 43: 'apothecary'}   # from shopkeeper NPC +0x74 (extract/out/rpg/shops.json)
INN_PRICE = {29: 100, 35: 200, 40: 500}   # 0x32242..0x32278, default 100

def block_for(room, kind):
    if kind == 'armory':
        return dict(ARMORY_BLOCKS).get(room, ARMORY_DEFAULT)
    return dict(APO_BLOCKS).get(room, APO_DEFAULT)

def parse_block(a):
    tm = [[be16(a + 2 * (y * 17 + x)) for x in range(17)] for y in range(5)]
    grid = list(R[a + 0xAF:a + 0xBA])
    return tm, grid

LIST_WIN = WINDOWS[4]
def cell_px(win, cx, cy):
    return [win['top_left_px'][0] + 8 * cx, win['top_left_px'][1] + 8 * cy]

shops = {}
icon_tiles = {}
for room, kind in SHOP_ROOMS.items():
    if kind == 'inn':
        shops[str(room)] = {'room': room, 'kind': 'inn', 'npc_shop_type': 1, 'price': INN_PRICE[room],
                            'price_ram': 'customer+0xD0 (re-set on every visit)',
                            'music_after': '0x%02X' % R[0x49DA + room]}
        continue
    a = block_for(room, kind)
    tm, grid = parse_block(a)
    items = []
    for s, rid in enumerate(grid):
        if rid == 0xFF:
            continue
        row, col = s // 4, s % 4
        bx, by = col * 10, row * 3
        icon = [tm[by][bx], tm[by][bx + 1], tm[by + 1][bx], tm[by + 1][bx + 1]]
        icon_tiles.setdefault(rid, icon)
        digits = ''.join(' ' if (w & 0x7FF) == 0x28A else chr(0x30 + (w & 0x7FF) - 0x280) for w in tm[by + 1][bx + 3:bx + 7])
        rec = records[rid]
        items.append({
            'slot': s, 'grid_row': row, 'grid_col': col, 'record': rid, 'name': rec['name'], 'class': rec['class'],
            'price': rec['price'] if rid != 3 else 'dynamic',
            'price_digits_in_block': digits,
            'cursor_cell': [5 + bx, 2 + by], 'icon_cell': [6 + bx, 2 + by], 'price_cells': [[9 + bx, 3 + by], 4],
            'cursor_px': cell_px(LIST_WIN, 5 + bx, 2 + by), 'icon_px': cell_px(LIST_WIN, 6 + bx, 2 + by),
            'price_px': cell_px(LIST_WIN, 9 + bx, 3 + by),
            'icon_tiles': [hx(w) for w in icon],
            'description_text_id': rec['text_id'],
        })
    shops[str(room)] = {'room': room, 'kind': kind, 'npc_shop_type': 2 if kind == 'armory' else 3,
                        'block_addr': hx(a, 5), 'block_tilemap': [[hx(w) for w in r] for r in tm],
                        'slot_grid': grid, 'items': items, 'music': '0x%02X' % R[0x49DA + room]}

# ---------------------------------------------------------------- equipment visuals (palette 0xC364, tiles 0x920C..0x95A4)
def pal_entry(slot, v):
    if slot == 'fighter_armor': return {'line': 0, 'indices': [9, 13], 'words': [hx(w, 3) for w in words(0xB814 + 10 * v, 5)], 'table': '0xB814 + 10*v'}
    if slot == 'mage_robe': return {'line': 0, 'indices': [4, 6], 'words': [hx(w, 3) for w in words(0xB868 + 6 * v, 3)], 'table': '0xB868 + 6*v'}
    t = {'fighter_helm': (0xB84C, 3), 'fighter_shield': (0xB83C, 5), 'fighter_weapon': (0xB800, 7), 'mage_wand': (0xB858, 9)}[slot]
    return {'line': 1, 'indices': [t[1], t[1] + 1], 'words': [hx(w, 3) for w in words(t[0] + 4 * v, 2)], 'table': '%s + 4*v' % hx(t[0], 5)}

def gfx_entry(slot, v):
    if slot == 'mage_wand':
        off = [0x0, 0x740, 0x0, 0x740][v]
        return {'vram': '0x0440', 'vram_tile': '0x022', 'bytes': '0x740', 'bank_offset': hx(off), 'jump_table': '0x95FA'}
    if slot == 'fighter_helm':
        off = [0xE80, 0x11C0, 0x1500][v]
        return {'vram': '0x0F60', 'vram_tile': '0x07B', 'bytes': '0x340', 'bank_offset': hx(off), 'jump_table': '0x960A'}
    if slot == 'fighter_shield':
        return ({'vram': '0x12A0', 'vram_tile': '0x095', 'bytes': '0x1E0', 'bank_offset': '0x1840', 'code': '0x9462'} if v
                else {'vram': '0x12A0', 'note': 'nothing loaded when no shield (tiles left as they were)'})
    if slot == 'fighter_weapon':
        off = [0x1A20, 0x1F20, 0x1F20, 0x1A20, 0x1F20][v]
        return {'vram': '0x1480', 'vram_tile': '0x0A4', 'bytes': '0x500', 'bank_offset': hx(off), 'jump_table': '0x9616'}
    return {'note': 'no tiles; palette only'}

EQUIP_NAMES = {}
for rec in records:
    if rec['kind'] == 'equipment':
        EQUIP_NAMES[(rec['slot'], rec['tier'])] = rec['name']
equip_vis = {}
for slot, (ta, stat, n) in BONUS.items():
    ram = [k for k, v in SLOT.items() if v == slot][0]
    vals = []
    for v in range(n):
        vals.append({'value': v, 'name': EQUIP_NAMES.get((slot, v), '(none)'), 'bonus': {stat: R[ta + v]},
                     'palette': pal_entry(slot, v), 'tiles': gfx_entry(slot, v)})
    equip_vis[slot] = {'ram': hx(ram, 6), 'values': vals}

# ---------------------------------------------------------------- pause window (0x2D270)
def cells_text(addr, n): return ''.join(chr(c) if 0x41 <= c <= 0x5A or 0x30 <= c <= 0x39 else {0x3A: ' ', 0x3C: '.', 0x5B: '/', 0x5C: '×'}.get(c, '?') for c in R[addr:addr + n])
pause = {
    'windows': {'p1': WINDOWS[13], 'p2': WINDOWS[14]},
    'open_code': '0x2D1AC (P1 window struct 0xFFED8A shape 13, P2 0xFFEFB8 shape 14), close 0x2D21E',
    'cells': [
        {'cell': [2, 0], 'len': 3, 'field': 'name', 'src': 'name of player (0xFF0A4E P1 / 0xFF0A51 P2)'},
        {'cell': [7, 0], 'text': 'LV', 'src': '0x2D4FC'},
        {'cell': [9, 0], 'len': 2, 'field': 'level', 'value': 'player+0x8C + 1', 'format': 'right-aligned, leading zero blank'},
        {'cell': [2, 2], 'text': cells_text(0x2D194, 4), 'src': '0x2D194'},
        {'cell': [2, 3], 'text': cells_text(0x2D198, 4)},
        {'cell': [2, 4], 'text': cells_text(0x2D19C, 4)},
        {'cell': [2, 5], 'text': cells_text(0x2D1A0, 4)},
        {'cell': [2, 6], 'text': cells_text(0x2D1A4, 4)},
        {'cell': [2, 7], 'text': cells_text(0x2D1A8, 4)},
        {'cell': [7, 2], 'len': 5, 'field': 'gold', 'value': 'player+0xCE'},
        {'cell': [7, 3], 'len': 5, 'field': 'exp', 'value': 'player+0x8A'},
        {'cell': [7, 4], 'len': 5, 'field': 'next_exp', 'value': 'player+0xDC (65535 at LV20)'},
        {'cell': [7, 5], 'len': 5, 'field': 'str', 'value': 'player+0x92'},
        {'cell': [7, 6], 'len': 5, 'field': 'ac', 'value': 'player+0x98'},
        {'cell': [7, 7], 'len': 5, 'field': 'agl', 'value': 'player+0x9A'},
        {'cell': [2, 9], 'text': cells_text(0x2D4FE, 14), 'src': '0x2D4FE'},
        {'cell': [2, 10], 'text': cells_text(0x2D4FE + 14, 14)},
        {'cell': [2, 11], 'text': cells_text(0x2D4FE + 28, 14)},
        {'cell': [16, 9], 'len': 1, 'field': 'herbs', 'value': 'player+0xD2'},
        {'cell': [16, 10], 'len': 1, 'field': 'antidotes', 'value': 'player+0xD4'},
        {'cell': [16, 11], 'len': 1, 'field': 'elixirs', 'value': 'player+0xD8'},
    ],
    'number_format': 'value -> 5 decimal digits (0x2C31A), the last N are shown; leading zeros become blank (glyph 0x3A, black) '
                     'except the last digit (0x2C298). Numbers are right-aligned in their field.',
    'not_shown': ['HP/MP (HUD only)', 'bells of life', 'lives', 'equipment', 'key items'],
}

lvgold = {
    'window': WINDOWS[8], 'struct': '0xFFEFB8', 'code': '0x32E7E',
    'cells': [
        {'cell': [2, 0], 'len': 3, 'field': 'customer name'},
        {'cell': [7, 0], 'text': 'LV', 'src': '0x33038'},
        {'cell': [9, 0], 'len': 2, 'field': 'customer level (+0x8C + 1)'},
        {'cell': [2, 2], 'text': 'GOLD', 'src': '0x3303A'},
        {'cell': [7, 2], 'len': 5, 'field': 'customer gold (+0xCE)', 'refresh': '0x32D2C after each payment'},
    ],
    'note': 'always shape 8 (left side) - also when P2 is the customer',
}
yesno = {
    'window': WINDOWS[5], 'struct': '0xFFED8A', 'code': '0x32DA4',
    'cells': [{'cell': [2, 1], 'text': ' YES', 'src': '0x3303E'}, {'cell': [2, 2], 'text': ' NO ', 'src': '0x33042'}],
    'cursor': {'cell_yes': [2, 1], 'cell_no': [2, 2], 'tile_p1': '0xC24D', 'tile_p2': '0xC24F', 'tables': '0x33046 -> 0x3304E (P1) / 0x33056 (P2)',
               'initial': 'YES', 'input': 'customer controller only (0x3306E: 0xFF0211 P1 / 0xFF0215 P2, new presses)',
               'up': 'YES', 'down': 'NO', 'move_sound': '0xB4 (only when the selection changes)',
               'confirm': 'B (sound 0xB4 if YES)', 'cancel': 'A or C = NO (silent)'},
}

data = {
    'source': 'extract/shops.py; spec docs/re/shops.md',
    'window_shapes': WINDOWS,
    'window_note': 'top_left_px = (cx-128-4w, cy-128-4h+8) from table 0x2CEA2; window drawn on plane A snapped to the 8-px grid; '
                   'cell (0,0) is the top-left border cell; text origin of the dialogue engine = cell (2,1) '
                   '(cell (2,9) in the shop list window when 0xFF0BBE < 0). Windows open/close with the grow/shrink animation '
                   '(+2 cols per frame, then +2 rows per frame).',
    'texts': {
        'inn': {'greeting_yesno': 95, 'rest': 96, 'no_money': 97, 'declined': 98, 'morning': 99},
        'armory': {'greeting_yesno': 100, 'what': 101, 'else_yesno': 102, 'bye': 103, 'no_money': 104},
        'apothecary': {'greeting_yesno': 107, 'what': 101, 'else_yesno': 102, 'bye': 108, 'no_money': 109},
        'common': {'who_is_this_for_2p': 105, 'cannot_use': 106, 'too_many': 110},
        'strings': {str(i): TEXT[i] for i in list(range(95, 111)) + list(range(111, 132)) + [138]},
    },
    'windows_used': {
        'lv_gold': 8, 'greeting': 9, 'yes_no': 5, 'list': 4, 'message': 6, 'inn_message_and_bye': 10,
    },
    'timing': {
        'glyph_frames': 2, 'b_speedup': False, 'glyph_sound': '0xB2 (non-space glyphs)',
        'ff_end_wait_frames': 81, 'ff_end_note': 'all shop windows are non-zero shapes: FF = auto-continue after 81 frames, no button',
        'inn_fade_out': '0x2990 (palette fade to black, see docs/re/ui_screens.md)', 'inn_dark_frames': 60, 'inn_jingle': '0x94',
        'inn_fade_in': '0x294A', 'inn_music_timer': '0xFF0A56 = 0xD7 -> room music 0x49DA[room] restarts 200 frames later',
        'buy_sound': '0x9C', 'cursor_sound': '0xB4',
    },
    'list_window': {
        'window': WINDOWS[4], 'block_cell': [6, 2], 'block_size': [17, 5],
        'cursor': {'start_slot': 0, 'start_cell': [5, 2], 'tile_p1': '0xC24D', 'tile_p2': '0xC24F', 'erase_tile': '0x828A',
                   'moves': {'up': {'slot': -4, 'cell': [0, -3]}, 'down': {'slot': 4, 'cell': [0, 3]},
                             'left': {'slot': -1, 'cell': [-10, 0]}, 'right': {'slot': 1, 'cell': [10, 0]}},
                   'sound': '0xB4 on every D-pad press (also when the move is refused)', 'wrap': False,
                   'refused': 'target slot byte 0xFF (grid is padded with 0xFF on all sides)',
                   'priority': 'up, down, left, right (first set bit wins)',
                   'keeps_position_after_purchase': True},
        'description': {'text_origin_cell': [2, 9], 'clear_area': [24, 6], 'restart_on_move': True,
                        'note': 'non-blocking typewriter (0xFF0BBE=-1); D-pad/B/A/C are read every frame while it types'},
        'extra_life_price': {'cell': [9, 6], 'digits': 4, 'code': '0x32CF6', 'rooms': [32]},
    },
    'records': records,
    'shops': shops,
    'lv_gold_window': lvgold,
    'yes_no_window': yesno,
    'who_window_2p': {'window': WINDOWS[6], 'text': 105, 'names_cell': [9, 4], 'names': ['P1 name row 4', 'P2 name row 5'],
                      'cursor_cell': [8, 4], 'initial': 'customer', 'skip_if': 'other player +0x7A >= 8 (dead/absent)',
                      'cancel': 'A/C -> leave shop (bye text)'},
    'pause_window': pause,
    'equipment_visuals': equip_vis,
    'equipment_visuals_note': 'Palette lines 0/1 are shared by both players (built by 0xC364 from 0xFF0AC0..0xFF0ACA); '
                              'equipment tiles: LZSS 0x68C90 (len 0xBF0) -> RAM 0xFF6DA4, then LZSS 0x639C4 (len 0xF8C) -> 0xFF8DA4 '
                              'and 0x420 bytes 0xFF9DA4 -> 0xFF8DA4: so bank offsets >= 0x2000 read font-bank bytes 0x1000.. '
                              '(relevant for weapon offset 0x1F20, 0x500 bytes).',
}
os.makedirs(os.path.join(GAME, 'data'), exist_ok=True)
json.dump(data, open(os.path.join(GAME, 'data', 'shops.ui.json'), 'w'), indent=1, ensure_ascii=False)

# ---------------------------------------------------------------- images
rm = rooms.load_room(30)
vram = bytearray(rm['vram'])
font = decompress(R, 0x639C4, 0xF8C)
vram[0x5000:0x5000 + 0x30 * 32] = font[0xD0 * 32:0x100 * 32]
pal = [[0] * 16 for _ in range(4)]
pal[2] = rm['pal'][2]
pal[0][14] = 0x000; pal[0][15] = 0xEEE

def draw(img, word, ox, oy, transparent0=True):
    t = word & 0x7FF; line = (word >> 13) & 3; hf = word & 0x800; vf = word & 0x1000
    for y in range(8):
        sy = 7 - y if vf else y
        for x in range(8):
            sx = 7 - x if hf else x
            b = vram[t * 32 + sy * 4 + (sx >> 1)]
            c = (b >> 4) if (sx & 1) == 0 else (b & 15)
            if c == 0 and transparent0:
                continue
            img.putpixel((ox + x, oy + y), cram_rgb(pal[line][c]) + (255,))

icons = Image.new('RGBA', (16 * 22, 16), (0, 0, 0, 0))
for rid, ws in icon_tiles.items():
    for k, w in enumerate(ws):
        draw(icons, w, rid * 16 + (k % 2) * 8, (k // 2) * 8)
os.makedirs(os.path.join(GAME, 'ui'), exist_ok=True)
icons.save(os.path.join(GAME, 'ui', 'shop_icons.png'))
cur = Image.new('RGBA', (16, 8), (0, 0, 0, 0))
draw(cur, 0xC24D, 0, 0); draw(cur, 0xC24F, 8, 0)
cur.save(os.path.join(GAME, 'ui', 'shop_cursors.png'))

# mock-ups ---------------------------------------------------------
BORDER = {'TL': 0x82AD, 'T': 0x82AE, 'TR': 0x8AAD, 'L': 0x82AF, 'F': 0x828A, 'R': 0x8AAF, 'BL': 0x92AD, 'B': 0x92AE, 'BR': 0x9AAD}
def glyph(ch):
    m = {' ': 0x3A, ',': 0x3B, '.': 0x3C, '!': 0x3D, "'": 0x3E, '?': 0x3F, '/': 0x5B, '×': 0x5C}
    return 0x8250 + m.get(ch, ord(ch))
def win(img, shape):
    w = WINDOWS[shape]; x0, y0 = w['top_left_px']
    for cy in range(w['h_cells']):
        for cx in range(w['w_cells']):
            k = ('T' if cy == 0 else 'B' if cy == w['h_cells'] - 1 else '') or ''
            if cy == 0: k = 'TL' if cx == 0 else 'TR' if cx == w['w_cells'] - 1 else 'T'
            elif cy == w['h_cells'] - 1: k = 'BL' if cx == 0 else 'BR' if cx == w['w_cells'] - 1 else 'B'
            else: k = 'L' if cx == 0 else 'R' if cx == w['w_cells'] - 1 else 'F'
            draw(img, BORDER[k], x0 + cx * 8, y0 + cy * 8, False)
    return x0, y0
def text(img, shape, cx, cy, s, base=None):
    x0, y0 = WINDOWS[shape]['top_left_px']
    for i, ch in enumerate(s):
        draw(img, glyph(ch) if base is None else base + ord(ch), x0 + (cx + i) * 8, y0 + cy * 8, False)
def num(v, n):
    s = '%05d' % v
    s = s[-n:]
    out = ''
    lead = True
    for i, ch in enumerate(s):
        if lead and ch == '0' and i < n - 1: out += ' '
        else: lead = False; out += ch
    return out
def lvgold_win(img, name='AAA', lv=1, gold=1234):
    win(img, 8); text(img, 8, 2, 0, name); text(img, 8, 7, 0, 'LV'); text(img, 8, 9, 0, num(lv, 2))
    text(img, 8, 2, 2, 'GOLD'); text(img, 8, 7, 2, num(gold, 5))
def textlines(img, shape, cx, cy, s):
    for k, line in enumerate(s.replace('{YESNO}', '').split('\n')):
        text(img, shape, cx, cy + k, line.replace('{NUM}', '  100'))
def yesno_win(img, sel=0, p=1):
    win(img, 5); text(img, 5, 2, 1, ' YES'); text(img, 5, 2, 2, ' NO ')
    x0, y0 = WINDOWS[5]['top_left_px']
    draw(img, 0xC24D if p == 1 else 0xC24F, x0 + 16, y0 + 8 * (1 + sel), False)

def bg():
    try:
        im = Image.open(os.path.join(ROOT, 'extract/out/rooms/room_30_fg.png')).convert('RGBA').crop((0, 0, 320, 224))
    except Exception:
        im = Image.new('RGBA', (320, 224), (40, 40, 40, 255))
    return im

os.makedirs(WORK, exist_ok=True)
for room, sh in shops.items():
    if sh['kind'] == 'inn':
        im = bg(); lvgold_win(im); win(im, 9); textlines(im, 9, 2, 1, TEXT[95]); yesno_win(im)
        im.save(os.path.join(WORK, 'mock_room%s_inn.png' % room)); continue
    im = bg(); lvgold_win(im); win(im, 9); textlines(im, 9, 2, 1, TEXT[100 if sh['kind'] == 'armory' else 107]); yesno_win(im)
    im.save(os.path.join(WORK, 'mock_room%s_greeting.png' % room))
    im = bg(); lvgold_win(im); win(im, 4)
    x0, y0 = WINDOWS[4]['top_left_px']
    for y, r in enumerate(sh['block_tilemap']):
        for x, w in enumerate(r):
            draw(im, int(w, 16), x0 + (6 + x) * 8, y0 + (2 + y) * 8, True)
    if int(room) == 32:
        text(im, 4, 9, 6, num(800, 4))
    draw(im, 0xC24D, x0 + 5 * 8, y0 + 2 * 8, False)
    textlines(im, 4, 2, 9, records[sh['items'][0]['record']]['text'])
    im.save(os.path.join(WORK, 'mock_room%s_list.png' % room))
im = bg(); lvgold_win(im); win(im, 4); win(im, 6); textlines(im, 6, 2, 1, TEXT[102]); yesno_win(im)
im.save(os.path.join(WORK, 'mock_else.png'))
print('records', len(records), 'shops', len(shops), 'icons', sorted(icon_tiles))
