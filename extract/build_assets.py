"""Converts extraction outputs (extract/out/*) into the game's asset formats (game/assets/*)."""
import json, os, shutil
ROOT = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(ROOT, 'out')
GAME = os.path.join(ROOT, '..', 'game', 'assets')

def font():
    d = json.load(open(os.path.join(OUT, 'ui', 'font.json')))
    os.makedirs(os.path.join(GAME, 'ui'), exist_ok=True)
    from PIL import Image
    img = Image.open(os.path.join(OUT, 'ui', 'font.png')).convert('RGBA')
    # The font was exported with pure white ink; the cartridge's white is CRAM 0xEEE.
    px = img.load()
    for y in range(img.height):
        for x in range(img.width):
            r, g, b, a = px[x, y]
            px[x, y] = (min(r, 238), min(g, 238), min(b, 238), a)
    img.save(os.path.join(GAME, 'ui', 'font.png'))
    glyphs = {}
    for g in d['glyphs']:
        key = g.get('byte') or g.get('name')
        glyphs[key] = [g['x'], g['y']]
    json.dump({'image': 'font.png', 'cell': 8, 'glyphs': glyphs}, open(os.path.join(GAME, 'ui', 'font.font.json'), 'w'), indent=1)
    print('font', len(glyphs))

def hud():
    d = json.load(open(os.path.join(OUT, 'ui', 'hud.json')))
    t = d['template']
    cells = [[{'g': c['byte'], 'h': c['hflip'], 'v': c['vflip']} for c in row] for row in t['cells']]
    out = {
        'rows': [24, 27], 'template': cells, 'template_cols': {'p1': 1, 'p2': 20},
        'name': d['name'], 'numbers': d['numbers']['fields'], 'bars': d['bars'],
        'no_player': 'NO PLAYER',
    }
    json.dump(out, open(os.path.join(GAME, 'ui', 'hud.hud.json'), 'w'), indent=1)
    print('hud ok')

def text():
    d = json.load(open(os.path.join(OUT, 'text', 'strings.json')))
    nodes = {}
    addrs = {int(n['addr'], 16) for n in d['nodes']}
    for n in d['nodes']:
        nxt = int(n['end'], 16) + 1
        toks = []
        for t in n['tokens']:
            t = dict(t)
            toks.append(t)
        last = toks[-1]['op'] if toks else None
        nodes[n['addr']] = {
            'tokens': toks,
            'next': ('0x%X' % nxt) if (last not in ('END', 'GOTO') and nxt in addrs) else None,
        }
    entries = {str(e['id']): e['addr'] for e in d['dialogue_table']['entries']}
    import sys; sys.path.insert(0, ROOT)
    from md import be16; from lzss import rom
    r = rom()
    windows = []
    for i in range(15):
        a = 0x2CEA2 + i * 8
        w, h, cx, cy = [be16(r, a + j) for j in (0, 2, 4, 6)]
        windows.append({'w': w, 'h': h, 'x': cx - 0x80 - w * 4, 'y': cy - 0x80 - h * 4 + 8})
    os.makedirs(os.path.join(GAME, 'text'), exist_ok=True)
    json.dump({'entries': entries, 'nodes': nodes, 'windows': windows, 'item_names': d['item_names'], 'fixed_strings': d['fixed_strings']},
              open(os.path.join(GAME, 'text', 'dialogue.text.json'), 'w'), indent=1)
    print('text', len(entries), 'entries', len(nodes), 'nodes')

def screens():
    src = os.path.join(OUT, 'screens'); dst = os.path.join(GAME, 'screens')
    files = {
        'sega': ['plane_a.png'] + [f'logo_cycle_{i:02d}.png' for i in range(21)],
        'title': ['plane_a.png', 'plane_b.png', 'press_start.png'],
        'charselect': ['plane_a.png', 'arrow.png'] + [f'select_frame_{i}.png' for i in range(5)],
        'nameentry': ['plane_a_1p.png', 'plane_b_1p.png', 'cursor_p1.png', 'plane_a_2p.png', 'plane_b_2p.png', 'cursor_p2.png'],
        'ending': ['plane_b.png', 'princess.png', 'princess_tinted.png', 'staff_roll.png', 'final_presented_by_taito.png', 'final_plane_a.png'],
        'continue_gameover': ['continue.png', 'credit.png'] + [f'digit_{i}.png' for i in range(10)] + [f'gameover_{i}.png' for i in range(8)],
    }
    for d, fs in files.items():
        os.makedirs(os.path.join(dst, d), exist_ok=True)
        for f in fs:
            shutil.copy(os.path.join(src, d, f), os.path.join(dst, d, f))
    print('screens copied')

def rooms():
    import sys; sys.path.insert(0, ROOT)
    import rooms as R
    from md import be16
    from PIL import Image
    src = os.path.join(OUT, 'rooms'); dst = os.path.join(GAME, 'rooms')
    os.makedirs(dst, exist_ok=True)
    for f in os.listdir(dst):
        os.remove(os.path.join(dst, f))
    for i in range(48):
        d = json.load(open(os.path.join(src, f'room_{i:02d}.json')))
        rm = R.load_room(i)
        for suffix in ('fg', 'fg_hi', 'bg'):
            shutil.copy(os.path.join(src, f'room_{i:02d}_{suffix}.png'), os.path.join(dst, f'room_{i:02d}_{suffix}.png'))
        # One white mask per cycling palette entry; the game tints it with the current colour.
        cycles = []
        for c in d.get('palette_cycles', []):
            line, idx = c['line'], c['index']
            for layer, only_hi in (('fg', False),) + ((('bg', None),) if True else ()):
                pass
            masks = {}
            for layer in ('fg', 'bg'):
                if layer == 'fg':
                    W, H = rm['W'] * 16, rm['H'] * 16
                    img = Image.new('RGBA', (W, H), (0, 0, 0, 0)); px = img.load()
                    for cy in range(rm['H']):
                        for cx in range(rm['W']):
                            cell = rm['cells'][cy * rm['W'] + cx]
                            for q in range(4):
                                w = be16(rm['mt'], cell * 8 + 2 * q)
                                mask_tile(px, rm['vram'], w, line, idx, cx * 16 + (q & 1) * 8, cy * 16 + (q >> 1) * 8)
                else:
                    img = Image.new('RGBA', (512, 256), (0, 0, 0, 0)); px = img.load()
                    for ty in range(32):
                        for tx in range(64):
                            mask_tile(px, rm['vram'], be16(rm['bg'], (ty * 64 + tx) * 2), line, idx, tx * 8, ty * 8)
                if img.getbbox():
                    name = f'room_{i:02d}_cycle_{line}_{idx}_{layer}.png'
                    img.save(os.path.join(dst, name))
                    masks[layer] = f'rooms/{name}'
            if masks:
                cycles.append({'masks': masks, 'steps': [{'frames': st['frames'], 'rgb': st['rgb']} for st in c['steps']]})
        exits = {'door': {}, 'edge': {}, 'warp': {}, 'stairs': {}}
        for e in d['exits']:
            if e.get('shadowed'):
                continue
            k = e['kind']; n = str(e['index'])
            if k == 'stairs':
                exits[k][n] = {'dy': e['dy_px'], 'freeze': e['lock_frames'], 'iframes': e['player_0x84']}
            elif 'dest_room' in e and e.get('dest_world_px'):
                exits[k][n] = {'room': e['dest_room'], 'x': e['dest_world_px'][0], 'y': e['dest_world_px'][1],
                               'facing': e['dest_facing'], 'camera': [e['dest_camera_blocks'][0] * 16, e['dest_camera_blocks'][1] * 16],
                               'music': e.get('music', 0), 'sound': e.get('sound')}
        out = {
            'index': i, 'blocks_w': d['width_blocks'], 'blocks_h': d['height_blocks'],
            'attrs': d['attrs'], 'backdrop': d['backdrop'],
            'fg': f'rooms/room_{i:02d}_fg.png', 'fg_hi': f'rooms/room_{i:02d}_fg_hi.png', 'bg': f'rooms/room_{i:02d}_bg.png',
            'bg_scroll': [d['bg_scroll']['x'], d['bg_scroll']['y']] if isinstance(d.get('bg_scroll'), dict) else [0.0, 0.0],
            'palette_cycles': cycles, 'music': d.get('music', 0), 'exits': exits,
            'spawns': d.get('events', []),
            'objects': f'objects/room_{i:02d}.objects.json',
        }
        json.dump(out, open(os.path.join(dst, f'room_{i:02d}.room.json'), 'w'))
    print('rooms built')
    # Scripted room events add images to game/assets/rooms/ (emptied above) – regenerate them.
    import room_events
    room_events.main()

def mask_tile(px, vram, word, line, idx, ox, oy):
    t = word & 0x7FF
    if t < 0x100 or ((word >> 13) & 3) != line:
        return
    hf = word & 0x800; vf = word & 0x1000; base = t * 32
    for y in range(8):
        sy = 7 - y if vf else y
        for x in range(8):
            sx = 7 - x if hf else x
            b = vram[base + sy * 4 + (sx >> 1)]
            c = (b >> 4) if (sx & 1) == 0 else (b & 15)
            if c == idx:
                px[ox + x, oy + y] = (255, 255, 255, 255)

if __name__ == '__main__':
    import sys
    which = sys.argv[1:] or ['font', 'hud', 'text', 'screens', 'rooms']
    for w in which:
        globals()[w]()
