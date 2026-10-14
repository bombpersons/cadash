"""Per-room object assets for the game: game/assets/objects/room_NN.objects.json + room_NN_objects.png.

For every room: the object types reachable in it (markers → spawn records → types, death types,
drops, script spawns; plus the shared types 0-15), their state scripts, every animation frame they
can show rendered with the room's own sprite bank and palette, the room's spawn records and markers.
Reuses the table parsers/renderer written for docs/re/objects.md.
"""
import os, sys, json
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from objtables import *  # noqa: F401,F403  (r, L, W, NROOMS, markers, pmarkers, records, ...)
import objects as O
from PIL import Image

GAME = os.path.join(HERE, '..', 'game', 'assets', 'objects')
ROOMS_JSON = os.path.join(HERE, 'out', 'objects', 'rooms.json')


def rec_json(rec):
    return {k: rec[k] for k in REC_FIELDS}


def pack(frames):
    """Shelf-pack frame images into an atlas; returns (atlas image, rects)."""
    W_ = 1024
    order = sorted(range(len(frames)), key=lambda i: -frames[i].height)
    rects = [None] * len(frames)
    x = y = rowh = 0
    for i in order:
        w, h = frames[i].size
        if x + w > W_:
            x = 0; y += rowh + 1; rowh = 0
        rects[i] = [x, y, w, h]
        x += w + 1; rowh = max(rowh, h)
    atlas = Image.new('RGBA', (W_, max(1, y + rowh)), (0, 0, 0, 0))
    for img, rc in zip(frames, rects):
        atlas.paste(img, (rc[0], rc[1]))
    return atlas, rects


# State-init opcodes that copy the current mapping into object RAM so scripts can move pieces.
# Behaviours that switch to anims no state names (see build_room).
SCRIPT_ANIM_BEHAVIOURS = {61, 63, 76, 78, 79}

RAM_MAPPING_INITS = {152, 154, 156, 158, 159, 194, 207, 208, 209, 210, 211, 212, 213, 214, 215, 231}


def render_piece(size, attr, vram, pal):
    """One mapping piece (size byte, attr word) as an RGBA image with its top-left at (0,0)."""
    w = ((size >> 2) & 3) + 1; h = (size & 3) + 1
    img = Image.new('RGBA', (8 * w, 8 * h), (0, 0, 0, 0)); px = img.load()
    t0 = attr & 0x7FF; line = (attr >> 13) & 3; hf = attr & 0x800; vf = attr & 0x1000
    for cx in range(w):
        for cy in range(h):
            tile = O.tile_pixels(vram, (t0 + cx * h + cy) & 0x7FF)
            tx = (w - 1 - cx) if hf else cx; ty = (h - 1 - cy) if vf else cy
            for y in range(8):
                for x in range(8):
                    c = tile[7 - y if vf else y][7 - x if hf else x]
                    if c:
                        px[tx * 8 + x, ty * 8 + y] = pal[line * 16 + c] + (255,)
    return img


PLANE_B_BOSS_MAP = 0x2AAA6


def palette_words(palsrc):
    """The 64 CRAM words a graphics context renders with: player lines 0/1, room/switch lines 2/3."""
    return O.player_palette() + [W(palsrc['line2'] + 2 * i) for i in range(16)] + [W(palsrc['line3'] + 2 * i) for i in range(16)]


def palette_atlases(idx_atlas, pal_ctx):
    """(colour atlas, indexed atlas) from an atlas of (index, context, 0, alpha) pixels.
    The indexed atlas is RGB: R = CRAM index 0..63 (0 = transparent), G = context id."""
    import numpy as np
    a = np.array(idx_atlas)
    lut = np.zeros((max(1, len(pal_ctx)), 64, 3), np.uint8)
    for k, (_, words) in enumerate(pal_ctx):
        lut[k] = [O.cram_rgb(w) for w in words]
    rgb = lut[np.minimum(a[..., 1], len(lut) - 1), a[..., 0] & 63]
    colour = np.dstack([rgb, a[..., 3]])
    colour[a[..., 3] == 0] = 0
    idx = a[..., :3].copy()
    idx[a[..., 3] == 0] = 0
    return Image.fromarray(colour, 'RGBA'), Image.fromarray(idx, 'RGB')


def S16(a):
    v = W(a)
    return v - 0x10000 if v & 0x8000 else v


def render_tilemap(addr, w, h, vram, pal):
    """Nametable words (row-major, w x h cells) as an RGBA image; tile 0x100 is the blank plane tile."""
    img = Image.new('RGBA', (8 * w, 8 * h), (0, 0, 0, 0)); px = img.load()
    for cy in range(h):
        for cx in range(w):
            e = W(addr + 2 * (cy * w + cx))
            t = e & 0x7FF; line = (e >> 13) & 3; hf = e & 0x800; vf = e & 0x1000
            if t == 0x100:
                continue
            tile = O.tile_pixels(vram, t)
            for y in range(8):
                for x in range(8):
                    c = tile[7 - y if vf else y][7 - x if hf else x]
                    if c:
                        px[cx * 8 + x, cy * 8 + y] = pal[line * 16 + c] + (255,)
    return img


def build_room(room, rooms_meta):
    tt = type_table(room); at = anim_table(room)
    recs = records(room); precs = records(room, True)
    reachable = set(int(a, 16) for a in rooms_meta[room].get('reachable_records', []))
    by_addr = {rc['addr']: rc for rc in recs + precs}
    variants = {(t, 0) for t in range(16) if t < type_count(tt)}
    for a in reachable:
        rc = by_addr.get(a)
        if rc is None:
            continue
        variants.add((rc['type'], rc['tile_ofs']))
        if rc['death_type'] < type_count(tt):
            variants.add((rc['death_type'], 0))
        if rc['gold'] & 0x8000:
            variants.add((rc['gold'] & 0xF, 0))
    # persistent records are always reachable through markers
    for rc in precs:
        variants.add((rc['type'], rc['tile_ofs']))
    nanim = anim_count(at)
    types = {}
    anims = {}
    frames = []
    frame_key = {}
    pieces = {}        # "attr:size:gfx" -> frame index (piece images for RAM-mapped objects)
    frame_pieces = {}  # frame index -> [[dx, dy, size, attr], ...]
    frame_piece0 = {}  # frame index -> [dx, dy] of the mapping's first piece (mapping+18/+20, read by B126/B130)
    uses_b72 = False
    uses_plane_b = False
    gfx_cache = {}
    pal_ctx = []       # [(context name, 64 CRAM words)] by context id

    def gfx(ctx):
        # Frames are rendered palette-indexed (pixel = (CRAM index, context id, 0)); the colour atlas
        # is derived from them at the end (see palette_atlases), the indexed one is kept for run-time
        # palette effects (docs/re/palette_effects.md).
        if ctx not in gfx_cache:
            vram, _pal, palsrc = O.gfx_for(room, ctx)
            k = len(pal_ctx)
            pal_ctx.append((ctx, palette_words(palsrc)))
            gfx_cache[ctx] = (vram, [(i, k, 0) for i in range(64)], palsrc)
        return gfx_cache[ctx]

    def render_anim(key, desc_addr, eff_ofs, ctx, ram_mapped):
        """Renders one anim descriptor (frames, boxes, RAM-mapping pieces) under `key`."""
        if key in anims:
            return
        vram, pal, _ = gfx(ctx)
        ad = anim_desc(desc_addr)
        fr = []
        for f in ad['frames']:
            fk = (f['mapping'], eff_ofs, ctx)
            if fk not in frame_key:
                img, ox, oy, _lines = O.render_mapping(f['mapping'], vram, pal, eff_ofs)
                m = mapping(f['mapping'])
                boxes = [m['hurt']['w'], m['hurt']['h'], m['hurt']['dx'], m['hurt']['dy'],
                         m['attack']['w'], m['attack']['h'], m['attack']['dx'], m['attack']['dy']]
                if img is None:
                    img = Image.new('RGBA', (1, 1), (0, 0, 0, 0)); ox = oy = 0
                frame_key[fk] = len(frames)
                frame_piece0[len(frames)] = [S16(f['mapping'] + 18), S16(f['mapping'] + 20)]
                frames.append((img, [ox, oy], boxes))
            fi = frame_key[fk]
            if ram_mapped and fi not in frame_pieces:
                m = mapping(f['mapping'])
                plist = []
                for pc in m['pieces']:
                    attr = (pc['attr'] + eff_ofs) & 0xFFFF
                    pk = '%d:%d:%s' % (attr, pc['size'], ctx)
                    if pk not in pieces:
                        pieces[pk] = len(frames)
                        frames.append((render_piece(pc['size'], attr, vram, pal), [0, 0], [0, 0, 0, 0, 0, 0, 30000, 30000]))
                    plist.append([pc['dx'], pc['dy'], pc['size'], attr])
                frame_pieces[fi] = {'count': W(f['mapping']), 'pieces': plist}
            fr.append({'frame': frame_key[fk], 'dur': f['duration']})
        anims[key] = {'loop': ad['loop'], 'coll': [ad['coll_w'], ad['coll_h']], 'frames': fr}

    for t, ofs in sorted(variants):
        if t >= type_count(tt):
            continue
        sp = L(tt + 4 * t)
        if not (0x1EEC2 <= sp < 0x21EEE):
            continue
        states = type_states(sp)
        if not states:
            continue
        ctxs = O.state_contexts(states, room, 'default')
        ram_mapped = any(sc['init'] in RAM_MAPPING_INITS for sc in states.values())
        tj = types.setdefault(str(t), {'name': O.type_name(tt, t), 'states': {}})
        for s, sc in states.items():
            tj['states'][str(s)] = {
                'anim': None if sc['anim'] == 0xFF else sc['anim'],
                'behaviour': sc['behaviour'], 'init': sc['init'],
                'gfx': ctxs[s], 'conds': [[c, n] for c, n in sc['conds']],
            }
            a = sc['anim']
            if a == 0xFF or a >= nanim:
                continue
            eff_ofs = ofs if a != 0 else 0
            render_anim('%d:%d:%s' % (a, eff_ofs, ctxs[s]), L(at + 4 * a), eff_ofs, ctxs[s], ram_mapped)
        behaviours = {sc['behaviour'] for sc in states.values()}
        inits = {sc['init'] for sc in states.values()}
        # Anims chosen at run time by script opcodes (not named by any state): render the room's whole
        # anim table for these types (B61 direction anims, B63/B76/B78/B79 anim = v160/v162 + v190).
        if behaviours & SCRIPT_ANIM_BEHAVIOURS:
            for c in sorted(set(ctxs.values())):
                for a in range(1, nanim):
                    render_anim('%d:%d:%s' % (a, ofs, c), L(at + 4 * a), ofs, c, ram_mapped)
        # I225/B114 path-following segments use direction anims outside the room table (0x1C472).
        if 225 in inits:
            for c in sorted(set(ctxs.values())):
                for d in range(9):
                    desc = L(0x1C472 + 4 * d)
                    render_anim('rom%X:0:%s' % (desc, c), desc, 0, c, False)
        if 72 in behaviours:
            uses_b72 = True
        if 112 in behaviours:
            uses_plane_b = True
    if uses_b72:
        vram, pal, _ = gfx('default')
        for pk in list(pieces):
            size = int(pk.split(':')[1])
            k2 = '%d:%d:default' % (0x20F8, size)
            if k2 not in pieces:
                pieces[k2] = len(frames)
                frames.append((render_piece(size, 0x20F8, vram, pal), [0, 0], [0, 0, 0, 0, 0, 0, 30000, 30000]))
    if uses_plane_b:
        # B112 draws Balrog's body as an 11x10-cell tilemap (0x2AAA6) on plane B and scrolls the plane so
        # its top-left is 40 px left of / 80 px above the object: exported as a one-frame anim.
        vram, pal, _ = gfx('default')
        img = render_tilemap(PLANE_B_BOSS_MAP, 11, 10, vram, pal)
        frames.append((img, [40, 80], [0, 0, 30000, 30000, 0, 0, 30000, 30000]))
        anims['planeB%X:0:default' % PLANE_B_BOSS_MAP] = {'loop': 0, 'coll': [0, 0], 'frames': [{'frame': len(frames) - 1, 'dur': 65530}]}
    atlas, rects = pack([f[0] for f in frames])
    atlas, idx_atlas = palette_atlases(atlas, pal_ctx)
    name = 'room_%02d_objects.png' % room
    atlas.save(os.path.join(GAME, name))
    idx_name = 'room_%02d_objects_idx.png' % room
    idx_atlas.save(os.path.join(GAME, idx_name))
    mk = []
    for m in markers(room):
        mk.append({'cell': [m['cx'], m['cy']], 'persistent': m['persistent'], 'high_pool': m['high_pool'],
                   'index': m['index'], 'guard_flag': None})
    for m in pmarkers(room):
        mk.append({'cell': [m['cx'], m['cy']], 'persistent': m['persistent'], 'high_pool': m['high_pool'],
                   'index': m['index'], 'guard_flag': m['guard_flag']})
    sheet = {
        'atlas': name,
        # Palette-indexed copy of the atlas and the CRAM words of each graphics context (G channel).
        'indexed': {'atlas': idx_name, 'contexts': [c for c, _ in pal_ctx], 'palettes': [w for _, w in pal_ctx]},
        'frames': [dict({'rect': rc, 'origin': f[1], 'boxes': f[2]}, **({'piece0': frame_piece0[i]} if i in frame_piece0 else {}))
                   for i, (f, rc) in enumerate(zip(frames, rects))],
        'anims': {k: {'loop': v['loop'], 'frames': v['frames']} for k, v in anims.items()},
        'pieces': pieces,
        'frame_pieces': {str(k): v for k, v in frame_pieces.items()},
    }
    json.dump(sheet, open(os.path.join(GAME, 'room_%02d.sheet.json' % room), 'w'))
    out = {
        'sheet': 'room_%02d.sheet.json' % room,
        'anim_collision': {k: v['coll'] for k, v in anims.items()},
        'types': types,
        'records': [rec_json(x) for x in recs], 'persistent_records': [rec_json(x) for x in precs],
        'markers': mk,
    }
    json.dump(out, open(os.path.join(GAME, 'room_%02d.objects.json' % room), 'w'))
    return len(types), len(anims), len(frames), atlas.size


def main():
    os.makedirs(GAME, exist_ok=True)
    meta = json.load(open(ROOMS_JSON))
    rooms = [int(a) for a in sys.argv[1:]] or range(NROOMS)
    for room in rooms:
        print(room, *build_room(room, meta))


if __name__ == '__main__':
    main()
