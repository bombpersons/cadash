#!/usr/bin/env python3
"""Export the Cadash UI font + HUD layout.

Outputs extract/out/ui/font.png (RGBA, opaque as in game), font_alpha.png (black bg transparent, for tinting),
font.json (char map), hud_tiles.png (HP/MP bar tiles), hud.json (HUD layout).
Sources (all LZSS, see extract/lzss.py):
  font  = LZSS 0x639C4 len 0xF8C, tiles 0xD0..0xFF -> VRAM tiles 0x280..0x2AF (DMA at 0x113A0 / 0x3E44);
          text byte b -> VDP tile 0x250+b, so glyph for byte b = bank tile 0xA0+b.
  arrow = LZSS 0x7D5A2 len 0x12D8 tile 0xE0 -> VRAM tile 0x290 in-game (overrides glyph 0x40 '(c)'; DMA at 0x3FF0)
  bars  = LZSS 0x64950 len 0x1196 tiles 0xC8..0xCF -> VRAM 0x378..0x37F (1..8 filled px)
"""
import json, os, sys, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lzss import decompress
from md import tile_pixels, cram_rgb
from PIL import Image
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
rom = open(os.path.join(ROOT, 'Cadash (USA, Korea) (En).md'), 'rb').read()
OUT = os.path.join(ROOT, 'extract/out/ui'); os.makedirs(OUT, exist_ok=True)

# palette line 1 (common/HUD) colours used by font tiles: 0 transparent, 1 red (HP bar), 14 black, 15 white
PAL = {0: (0, 0, 0, 0), 1: cram_rgb(0x00E) + (255,), 14: cram_rgb(0x000) + (255,), 15: cram_rgb(0xEEE) + (255,)}
def col(i, alpha_bg=False):
    if alpha_bg and i == 14: return (0, 0, 0, 0)
    return PAL.get(i, (255, 0, 255, 255))

font = decompress(rom, 0x639C4, 0xF8C)
arrowbank = decompress(rom, 0x7D5A2, 0x12D8)
hud = decompress(rom, 0x64950, 0x1196)

GLYPH_NAMES = {0x3A: ' ', 0x3B: ',', 0x3C: '.', 0x3D: '!', 0x3E: "'", 0x3F: '?', 0x40: '©', 0x5B: '/', 0x5C: '×',
               0x5D: 'BORDER_CORNER_TL', 0x5E: 'BORDER_TOP', 0x5F: 'BORDER_LEFT'}
cells = []  # (key, data, tile)
for b in range(0x30, 0x60):
    cells.append((b, font, 0xA0 + b))
cells.append(('PROMPT_ARROW', arrowbank, 0xE0))
for i in range(8):
    cells.append(('BAR_%d' % (i + 1), hud, 0xC8 + i))

COLS = 16
def render(alpha_bg):
    rows = (len(cells) + COLS - 1) // COLS
    im = Image.new('RGBA', (COLS * 8, rows * 8))
    for k, (_, data, t) in enumerate(cells):
        px = tile_pixels(data, t)
        for y in range(8):
            for x in range(8):
                im.putpixel(((k % COLS) * 8 + x, (k // COLS) * 8 + y), col(px[y][x], alpha_bg))
    return im
render(False).save(os.path.join(OUT, 'font.png'))
render(True).save(os.path.join(OUT, 'font_alpha.png'))

glyphs = []
for k, (key, data, t) in enumerate(cells):
    g = {'cell': k, 'x': (k % COLS) * 8, 'y': (k // COLS) * 8, 'w': 8, 'h': 8}
    if isinstance(key, int):
        ch = GLYPH_NAMES.get(key, chr(key))
        g.update({'byte': '0x%02X' % key, 'char': ch, 'vram_tile': '0x%03X' % (0x250 + key), 'src': 'LZSS 0x639C4 tile 0x%02X' % t})
    else:
        g.update({'name': key, 'src': ('LZSS 0x7D5A2 tile 0xE0 (in-game VRAM tile 0x290)' if key == 'PROMPT_ARROW'
                                        else 'LZSS 0x64950 tile 0x%02X (VRAM tile 0x%03X)' % (t, 0x2B0 + t))})
    glyphs.append(g)
json.dump({
    'image': 'font.png', 'image_alpha_bg': 'font_alpha.png', 'cell': [8, 8], 'columns': COLS,
    'colors': {'background(idx14)': '#000000', 'ink(idx15)': '#FFFFFF', 'bar(idx1)': '#FF0000'},
    'encoding_note': 'game text byte b (0x30..0x5F) -> glyph; letters/digits are ASCII; 0x3A space, 0x3B comma, 0x3C period, 0x3D !, 0x3E apostrophe. '
                     'Window border: TL corner 0x5D, top 0x5E, left 0x5F; other edges/corners are the same tiles flipped '
                     '(TR = 0x5D hflip, BL = 0x5D vflip, BR = 0x5D hvflip, right = 0x5F hflip, bottom = 0x5E vflip). Window fill = 0x3A (space, solid black).',
    'monospace_advance': 8, 'line_height': 8,
    'glyphs': glyphs}, open(os.path.join(OUT, 'font.json'), 'w'), indent=1)

# ---------------- HUD layout -----------------
def be16(o): return struct.unpack('>H', rom[o:o + 2])[0]
tmpl = [[be16(0x8FB0 + 2 * (r * 19 + c)) for c in range(19)] for r in range(4)]
def cell_desc(w):
    t = w & 0x7FF; s = ''
    if 0x280 <= t < 0x2B0: s = GLYPH_NAMES.get(t - 0x250, chr(t - 0x250))
    return {'tile': '0x%03X' % t, 'byte': '0x%02X' % (t - 0x250) if 0x280 <= t < 0x2B0 else None, 'glyph': s,
            'pal': (w >> 13) & 3, 'hflip': bool(w & 0x800), 'vflip': bool(w & 0x1000), 'prio': bool(w & 0x8000)}
template_text = [''.join(cell_desc(w)['glyph'] if cell_desc(w)['glyph'] and len(cell_desc(w)['glyph']) == 1 else '#' for w in row) for row in tmpl]
def pos(vaddr):
    o = vaddr - 0xF000
    return {'col': (o % 0x80) // 2, 'row': o // 0x80, 'x': (o % 0x80) // 2 * 8, 'y': o // 0x80 * 8}
hudj = {
    'plane': 'VDP window plane, nametable base 0xF000 (64 cells/row); HUD occupies screen rows 24..27 (y 192..223)',
    'draw_routine': '0x8992 (init: clears rows 24..27 with 0x828A, draws template, names); per-frame update 0x8A4E',
    'template': {'addr': '0x8FB0', 'w': 19, 'h': 4, 'rows_text': template_text,
                 'cells': [[cell_desc(w) for w in row] for row in tmpl],
                 'p1_origin': pos(0xFC02), 'p2_origin': pos(0xFC28),
                 'p2_drawn_if': 'word 0xFF0A40 != 0 (P2 joined); otherwise "NO PLAYER" (ROM 0x9048) at ' + json.dumps(pos(0xFCB2))},
    'name': {'p1': pos(0xFC06), 'p2': pos(0xFC2C), 'len': 3, 'src_ram': {'p1': '0xFF0A4E', 'p2': '0xFF0A51'}, 'palette': 0},
    'numbers': {
        'routine': '0x8B14 (binary->decimal 0x2C31A into 0xFF0B8A.., printed by 0x2C298 with tile base 0x8280, pal 0)',
        'fields': [
            {'name': 'level', 'player_offset': '0x8C', 'display': 'value+1', 'digits': 2, 'p1': pos(0xFC14), 'p2': pos(0xFC3A)},
            {'name': 'hp', 'player_offset': '0x90', 'digits': 3, 'p1': pos(0xFC98), 'p2': pos(0xFCBE)},
            {'name': 'hp_max', 'player_offset': '0x8E', 'digits': 3, 'p1': pos(0xFCA0), 'p2': pos(0xFCC6)},
            {'name': 'mp', 'player_offset': '0x96', 'digits': 3, 'p1': pos(0xFD18), 'p2': pos(0xFD3E)},
            {'name': 'mp_max', 'player_offset': '0x94', 'digits': 3, 'p1': pos(0xFD20), 'p2': pos(0xFD46)},
        ],
        'note': 'player struct P1 0xFF0DC0, P2 0xFF0F40; leading zeros blanked (right aligned)'},
    'bars': {
        'routine': '0x8B62 (HP) / 0x8BEE (MP)',
        'hp': {'p1': pos(0xFC88), 'p2': pos(0xFCAE), 'value': 'player+0x90'},
        'mp': {'p1': pos(0xFD08), 'p2': pos(0xFD2E), 'value': 'player+0x96'},
        'width_cells': 8, 'px_per_point': 0.25,
        'formula': 'px = ceil(value/4); full cells tile 0xA37F (8px), then one partial tile 0xA377+(px%8) (1..7px), rest blank 0x828A',
        'tiles': 'font.png glyphs BAR_1..BAR_8 (red = palette 1 idx 1, 0x00E)'},
    'messages': {
        'engine': '0x8C9A, message id = word player+0xB2 (queued id at +0xB4), text = dialogue_table entry 138+id (table 0x2D750)',
        'area': {'p1': pos(0xFC84), 'p2': pos(0xFCAA), 'w': 17, 'h': 2, 'clear_tile': '0x828A'},
        'speed': '1 character per frame', 'page_wait_frames': 60, 'end_wait_frames': '60 for FE; FF ends immediately',
        'after': 'HUD template/name redrawn (0x8EC8..)',
        'codes': 'F4 = this player name, F3 = digit of 0xFF0A7A (continues/lives?), F2 ptr: if 0xFF0A7A==0 goto, F1 ptr: if 0xFF0206!=0 goto'},
    'hidden_when': 'player state word +0x7A >= 8 (numbers/bars not refreshed)',
}
json.dump(hudj, open(os.path.join(OUT, 'hud.json'), 'w'), indent=1)
print('\n'.join(template_text))
