"""Build the non-gameplay screens of Cadash from ROM data into extract/out/screens/<screen>/.
See docs/re/ui_screens.md.  Run: python3 extract/screens.py"""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from screens_vdp import *
from PIL import Image

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'out', 'screens')
PLANE_A, PLANE_B = 0xC000, 0xE000

def odir(name):
    d = os.path.join(OUT, name); os.makedirs(d, exist_ok=True); return d

def save(img, d, fn):
    img.save(os.path.join(d, fn)); return fn

def hexs(ws): return ['0x%03X' % w for w in ws]

def nt_pos(addr, base):
    off = (addr - base)//2; return dict(col=off % 64, row=off//64, x=(off % 64)*8, y=(off//64)*8)

def composite(layers):
    img = Image.new('RGBA', (320, 224), (0, 0, 0, 255))
    for l in layers: img.alpha_composite(l)
    return img

# ---------------------------------------------------------------- SEGA logo (0x1D86)
def sega():
    d = odir('sega')
    vm = VRAM()
    vm.put(0x20, ROM[0x1F52:0x1F52+0x620])          # 0x310 words raw -> VRAM 0x0020 (tiles 1..49)
    nt_us = words(ROM, 0x25DA, 13*4); nt_jp = words(ROM, 0x2572, 13*4)
    vm.put_nt(0xC61C, nt_us, 13, 4)
    pal = words(ROM, 0x1ED2, 64)
    cyc = words(ROM, 0x1E94, 31)
    a = render_plane(vm, PLANE_A, pal)
    save(a, d, 'plane_a.png'); save(composite([a]), d, 'composite.png')
    logo = render_rect(vm, PLANE_A, pal, 14, 12, 13, 4); save(logo, d, 'logo.png')
    # without the TM (Japanese table 0x2572)
    vm2 = VRAM(); vm2.m[:] = vm.m; vm2.put_nt(0xC61C, nt_jp, 13, 4)
    save(render_rect(vm2, PLANE_A, pal, 14, 12, 13, 4), d, 'logo_no_tm.png')
    frames = []
    for i, off in enumerate(range(0x28, -1, -2)):      # 21 steps, one every 4 frames
        p = list(pal); p[2:13] = cyc[off//2: off//2 + 11]
        fn = 'logo_cycle_%02d.png' % i
        save(render_rect(vm, PLANE_A, p, 14, 12, 13, 4), d, fn)
        frames.append(dict(file=fn, palette_line0_colours_2_12=hexs(p[2:13])))
    return dict(
        code='0x1D86 (called from reset path 0x39C before main state machine 0x3BE2)',
        tiles=dict(rom=0x1F52, bytes=0x620, format='raw 4bpp', vram=0x0020, first_tile=1, count=49),
        nametable=dict(rom_overseas=0x25DA, rom_japan=0x2572, cells='13x4', select='version reg 0xA10001 bit7 (overseas -> 0x25DA, has TM)',
                       vram=0xC61C, plane='A', **nt_pos(0xC61C, PLANE_A)),
        palette=dict(rom=0x1ED2, words=64, colours_line0=hexs(pal[:16])),
        layers=dict(plane_a='plane_a.png', composite='composite.png', logo='logo.png', logo_position=[112, 96],
                    logo_no_tm='logo_no_tm.png'),
        palette_cycle=dict(table_rom=0x1E94, table=hexs(cyc), dest='palette line 0 colours 2..12 (0xFF0274)',
                           window_words=11, start_word_offset=20, step_words=-1, steps=21, frames_per_step=4,
                           note='single sweep of a bright band across the logo (counter 0xFF0220 reloaded with 3, offset 0xFF0222 from 0x28 down by 2 until negative); afterwards palette stays at final step (== base palette)',
                           frames=frames),
        timing=dict(palette_in='instant (0x28D6 copies target->current, no fade)', hold_frames=0x12B,
                    skip='any of A/B/C/START new-press (0xFF0211/0xFF0215 & 0xF0) ends early',
                    fade_out='0x2990 standard fade', measured='screen visible from frame ~41, title state entered frame 373 (no input)'),
        next='title (main state 0xFF0A42 = 0)')

# ---------------------------------------------------------------- Title (0x10AF8)
def wave_table(amp, period=0x20):
    """replicates 0x11250 + 0x111CE: piecewise-parabolic sine, 4*period+1 entries (px, signed)"""
    st = dict(c=0, ph=0)
    def step(d4, d5):
        if st['ph'] & 1: d5 -= amp
        else: d5 += amp
        if st['ph'] & 2: d4 -= d5
        else: d4 += d5
        st['c'] += 1
        if st['c'] >= period:
            st['c'] = 0; st['ph'] ^= 1
            if not (st['ph'] & 1): st['ph'] ^= 2
        return d4 & 0xFFFFFFFF if d4 >= 0 else d4, d5
    # 0x11250: run until bit1 of phase set, d4 = -(d4/2)
    d4 = d5 = 0
    while True:
        d4, d5 = step(d4, d5)
        if st['ph'] & 2: break
    d4 = -(d4 >> 1)
    st['c'] = 0; st['ph'] = 0; d5 = 0
    tab = []
    for _ in range(period*4 + 1):
        d4, d5 = step(d4, d5)
        v = (d4 >> 16) & 0xFFFF
        tab.append(v - 0x10000 if v & 0x8000 else v)
    return tab

def title():
    d = odir('title')
    vm = VRAM()
    vm.put(0x0000, lz(0x124D2, 0xA90)[:0x2000])        # logo + TAITO tiles -> VRAM 0 (tiles 0..0xFF)
    fb = lz(0x639C4, 0xF8C); vm.put(0x2000, fb[0x1400:0x2000])  # font bank tiles 0xA0..0xFF -> VRAM 0x2000 (tile 0x100..)
    nta = lz(0x12F62, 0x368)
    vm.put_nt(0xC000, words(nta, 0, 40*28), 40, 28)
    ntb = words(ROM, 0x132CA, 34*10)
    vm.put_nt(0xE286, ntb, 34, 10)
    pal = words(ROM, 0x11278, 64)
    a = render_plane(vm, PLANE_A, pal); b = render_plane(vm, PLANE_B, pal)
    save(a, d, 'plane_a.png'); save(b, d, 'plane_b.png')
    logo, bb = bbox_crop(b); save(logo, d, 'logo.png')
    # PRESS START BUTTON sprite (object 0xFF10C0 anim 0x10F28, mapping 0x10F38, pos x=0xD8 y=0x100)
    pcs = mapping_pieces(0x10F38)
    ps = render_pieces(vm, pcs, pal, 0xD8-0x80, 0x100-0x80)
    save(ps, d, 'sprites.png')
    psc, psbb = bbox_crop(ps); save(psc, d, 'press_start.png')
    save(composite([b, a, ps]), d, 'composite.png')
    # wave tables for the ripple
    waves = {hex(a_): wave_table(a_) for a_ in (0x1500, 0x240)}
    # ripple preview frames (intro start, idle)
    def ripple(amp, linestep, phase, planes):
        tab = wave_table(amp) if amp else [0]*129
        L = len(tab); img = Image.new('RGBA', (320, 224), (0, 0, 0, 255))
        ph = phase
        for y in range(224):
            idx = (ph >> 16)
            hs = tab[idx % L]
            for layer, bit in ((b, 2), (a, 1)):
                sx = hs if planes & bit else 0
                row = layer.crop((0, y, 320, y+1))
                img.alpha_composite(row, ((sx) % 320, y)); img.alpha_composite(row, ((sx) % 320 - 320, y))
            ph += linestep
            if (ph >> 16) > L - 1: ph &= 0xFFFF
        return img
    save(ripple(0x1500, 0x90000, 0, 3), d, 'preview_intro_frame0.png')
    save(ripple(0x240, 0x30000, 0, 2), d, 'preview_idle.png')
    return dict(
        code='0x10AF8 (main state 0)',
        tiles=[dict(rom=0x124D2, lzss_len=0xA90, vram=0x0000, tiles='0x000-0x0FF', what='CADASH flame logo + TAITO logo'),
               dict(rom=0x639C4, lzss_len=0xF8C, part='bank tiles 0xA0..0xFF (0x600 words from +0x1400)', vram=0x2000,
                    tiles='0x100-0x15F', what='font: VDP tile = 0x100 + ASCII')],
        plane_a=dict(rom=0x12F62, lzss_len=0x368, cells='40x28', vram=0xC000, what='TAITO logo, (c) TAITO 1992, LICENSED BY SEGA ENTERPRISES LTD.', file='plane_a.png'),
        plane_b=dict(rom=0x132CA, format='raw words', cells='34x10', vram=0xE286, **nt_pos(0xE286, PLANE_B), what='CADASH logo', file='plane_b.png',
                     logo_crop='logo.png', logo_bbox=list(bb)),
        palette=dict(rom=0x11278, words=64, note='0x10FC4 copies 128 words (current+target) for the outro'),
        sprites=dict(press_start=dict(file='press_start.png', bbox=list(psbb), object='0xFF10C0', anim=0x10F28, mapping=0x10F38,
                                      pos_vdp=[0xD8, 0x100], pos_screen=[0xD8-0x80, 0x100-0x80],
                                      blink=dict(visible_frames=60, hidden_frames=10, hidden_mapping=0x10FBA),
                                      tiles='font tile 0x100+ASCII, palette 1')),
        ripple=dict(
            description='per-line horizontal scroll (VDP reg 0x8B03 line-scroll; table 0xFF0640 = {A,B} words x 256 lines, built by 0x110B0). Wave = piecewise-parabolic sine of 4*period+1 entries rebuilt by 0x11162 when amplitude/period change.',
            vars=dict(line_step='0xFFF514 (16.16 table entries per scanline)', amplitude='0xFFF518', period='0xFFF51A (=0x20, wave len 129)',
                      phase='0xFFF524', phase_speed='0xFFF52A (0x80000 = 8 entries/frame)', planes='0xFFF528 bit0 = plane A, bit1 = plane B'),
            wave_tables=waves,
            phases=[dict(name='intro', loop=0x10D0E, line_step_start=0x90000, line_step_delta=-0x1000, amplitude_start=0x1500,
                         amplitude_delta=-0x30, planes=3, ends='amplitude reaches 0 (112 frames) or START (either pad) -> idle',
                         palette='fade in from black: one 0x29C4 step every 5 frames (0xFFF532), components B,G,R in rotation, 1 level (0x2) per step'),
                    dict(name='idle', loop=0x10DE8, line_step=0x30000, amplitude=0x240, planes=2, phase_speed=0x80000, frames=0xA8C,
                         note='only the CADASH logo (plane B) keeps rippling gently; PRESS START blinks; cheat input recorder 0x112F8 runs'),
                    dict(name='outro', loop=0x10EA0, line_step_start=0, line_step_delta=0x1000, amplitude_start=0, amplitude_delta=0x30,
                         planes=3, ends='amplitude == 0x1500 (112 frames) then 0x2990 fade', palette='fade out one 0x2A30 step every 5 frames')],
            previews=['preview_intro_frame0.png', 'preview_idle.png']),
        input=dict(start='START new press on either pad (0xFF0211/0xFF0215 bit7): during intro or outro -> jump to idle phase (0x10D5A, palette set instantly by 0x10FC4, ripple reduced to idle values); during idle -> sound 0x97, wait 61 frames, fade out (0x2990), main state 2 (character select)',
                   cheat=dict(recorder=0x112F8, sequence='UP DOWN C A LEFT RIGHT B C (first 8 pad-1 presses on the title, bytes 01 02 20 40 04 08 10 20)',
                              effect='0xFF0206 = 0xFFFF, sound 0xA1. Infinite continues (0x9E2A/0x2BFA4 skip credit check) and other debug behaviour')),
        timing=dict(intro_frames=112, idle_frames=0xA8C, outro_frames=112, total_measured='title entered frame 373, demo state set at frame 3338 (no input)',
                    music='sound 0x81 queued by state 0 before 0x10AF8'),
        next='START -> character select; timeout -> attract demo (main state 1)')


# ---------------------------------------------------------------- text windows (engine documented in docs/re/ui_rpg.md)
WIN = dict(TL=0x82AD, T=0x82AE, TR=0x8AAD, L=0x82AF, F=0x828A, R=0x8AAF, BL=0x92AD, B=0x92AE, BR=0x9AAD)
def window_shape(i):
    w, h, cx, cy = words(ROM, 0x2CEA2 + i*8, 4)
    return dict(shape=i, cells=[w, h], x=cx - 0x80 - w*4, y=cy - 0x80 - h*4 + 8)
def window_cells(w, h):
    c = []
    for y in range(h):
        for x in range(w):
            if y == 0: c.append(WIN['TL'] if x == 0 else WIN['TR'] if x == w-1 else WIN['T'])
            elif y == h-1: c.append(WIN['BL'] if x == 0 else WIN['BR'] if x == w-1 else WIN['B'])
            else: c.append(WIN['L'] if x == 0 else WIN['R'] if x == w-1 else WIN['F'])
    return c
def render_window(vm, pal, w, h, texts=()):
    """texts: (col,row,bytes,base) inside the window"""
    tmp = VRAM(); tmp.m[:] = vm.m
    tmp.put_nt(0xC000, window_cells(w, h), w, h)
    for col, row, s, base in texts: tmp.put_text(0xC000 + (row*64 + col)*2, s, base)
    return render_rect(tmp, PLANE_A, pal, 0, 0, w, h)

def cycle_table(tab, room):
    """palette-cycle sets (0x2C398 loader / 0x2C3DA ticker): {w n; n x {w cram byte offset, l script}}; script {w dur,w colour}..0xFFFF"""
    a = be32(ROM, tab + room*4); n = be16(ROM, a); o = a + 2; out = []
    for i in range(n):
        off = be16(ROM, o); s = be32(ROM, o+2); o += 6; seq = []
        while be16(ROM, s) != 0xFFFF: seq.append(dict(frames=be16(ROM, s) + 1, colour='0x%03X' % be16(ROM, s+2))); s += 4
        out.append(dict(palette_index=off//2, line=off//32, colour=(off//2) % 16, script=s and hex(be32(ROM, o-4)), steps=seq))
    return hex(a), out

# ---------------------------------------------------------------- Character select (0x6340, main state 2)
def charselect():
    d = odir('charselect')
    vm = VRAM()
    vm.put(0x0000, lz(0x778A, 0x1208)[:0x2000])
    font_vram(vm)
    vm.put_nt(0xC212, words(ROM, 0x7348, 22*14), 22, 14)
    pal = words(ROM, 0x7266, 64)
    a = render_plane(vm, PLANE_A, pal)
    save(a, d, 'plane_a.png')
    # portraits individually (cells of NT 0x7348: fighter cols 0..8, mage cols 13..21, rows 2..13)
    port = render_rect(vm, PLANE_A, pal, 9, 4, 22, 14); save(port, d, 'portraits_block.png')
    # 1P/2P window (shape 11) with text 0x7246 split at ':'
    sh11 = window_shape(11); sh12 = window_shape(12)
    menu = render_window(vm, pal, 12, 4, [(3, 1, ROM[0x7246:0x724E], 0x250), (3, 2, ROM[0x724E:0x7256], 0x250)])
    save(menu, d, 'menu_window.png')
    msg = render_window(vm, pal, 26, 8); save(msg, d, 'message_window.png')
    arrow = Image.new('RGBA', (8, 8), (0, 0, 0, 0)); draw_tile(arrow, vm.m, 0xB8, 0, 0, pal_rgb(pal)[48:64])
    save(arrow, d, 'arrow.png')
    # selection frame sprite, 5 palette-cycle frames
    rid, cyc = cycle_table(0x30BDC, 0)
    pcs = mapping_pieces(0x72E6)
    frames = []
    for k in range(5):
        p = list(pal)
        for c in cyc: p[c['palette_index']] = int(c['steps'][k]['colour'], 16)
        img = Image.new('RGBA', (320, 224), (0, 0, 0, 0))
        render_pieces(vm, pcs, p, 0xD0-0x80, 0xB8-0x80, img)
        cr = img.crop((0xD0-0x80, 0xB8-0x80, 0xD0-0x80+80, 0xB8-0x80+96)); bb = cr.getbbox()
        fn = 'select_frame_%d.png' % k; save(cr, d, fn); frames.append(fn)
    comp = composite([a]); comp.alpha_composite(menu, (sh11['x'], sh11['y'])); comp.alpha_composite(arrow, (sh11['x']+16, sh11['y']+8))
    save(comp, d, 'composite_menu.png')
    spl = Image.new('RGBA', (320, 224), (0, 0, 0, 0)); render_pieces(vm, pcs, pal, 0xD0-0x80, 0xB8-0x80, spl); save(spl, d, 'sprites.png')
    comp2 = composite([a]); comp2.alpha_composite(msg, (sh12['x'], sh12['y']))
    fr = Image.open(os.path.join(d, frames[0])); comp2.alpha_composite(fr, (0xD0-0x80, 0xB8-0x80))
    save(comp2, d, 'composite_select.png')
    return dict(
        code='0x6340 (main state 2; also runs name entry 0x6776.. then main state 3)',
        tiles=[dict(rom=0x778A, lzss_len=0x1208, vram=0x0000, tiles='0x000-0x0FF', what='portraits, frames, arrow tile 0xB8 (P1) / 0xBA (P2), select-frame tiles 0x8B..0x9B, name-entry red squares'),
               dict(rom=0x639C4, lzss_len=0xF8C, part='bank tiles 0xD0..0xFF', vram=0x5000, tiles='0x280-0x2AF', what='font, text tile = byte + 0x250')],
        plane_a=dict(rom=0x7348, format='raw words', cells='22x14', vram=0xC212, **nt_pos(0xC212, PLANE_A),
                     what='SELECT THE PLAYER! + FIGHTER / MAGE portraits', file='plane_a.png', crop='portraits_block.png'),
        palette=dict(rom=0x7266, words=64, fade_in='0x293C (one component step every 2 frames)', music='sound 0x82'),
        menu_window=dict(file='menu_window.png', shape=sh11, open_anim='text-engine window open 0x2CA6E (see ui_rpg.md), window struct 0xFFED8A',
                         text_rom=0x7246, text='1PLAYER / 2PLAYERS at window col 3, rows 1-2',
                         cursor=dict(file='arrow.png', tile='0x60B8 (pal 3, tile 0xB8)', blank='0x828A', table=0x7256,
                                     window_cell=[2, 1], rows='row 1 = 1PLAYER, row 2 = 2PLAYERS', draw=0x6952),
                         input='UP/DOWN (pad1 new-press bits 0/1, only when 0xFF0209 == 0x0D i.e. 3-button pad id?) move, sound 0xB4; B (bit4) selects -> 0xFF0A40 = 0/1 (players-1)'),
        message_window=dict(file='message_window.png', shape=sh12,
                            messages={'after 1P/2P': 'dialogue id 0x84 (ONE PLAYER.) / 0x85 (two players)',
                                      'while choosing': 'id 0x86 (fighter description) / 0x87 (mage description) re-printed on every change',
                                      'note': 'ids index table 0x2D528 (see extract/out/text/strings.json)'}),
        select_cursor=dict(object='0xFF0DC0', mapping=0x72E6, frames=frames, pos_vdp={'fighter': [0xD0, 0xB8], 'mage': [0x138, 0xB8]},
                           pos_screen={'fighter': [0x50, 0x38], 'mage': [0xB8, 0x38]},
                           palette_cycle=dict(table=0x30BDC, room=0, set=rid, ticker=0x2C3DA, entries=cyc,
                                              note='line 3 colours 1..5 rotate; each step lasts 4 frames (counter 3..-1)')),
        input=dict(left_right='pad1 LEFT/RIGHT (bits 2/3) moves frame between FIGHTER (0xD0) and MAGE (0x138), sound 0xB4',
                   b='B confirms: 0xFF0A44 = P1 class (0 fighter/1 mage), 0xFF0A46 = P2 class (the other one), sound 0xB4, window closes, fade 0x2990',
                   two_players='in 2P mode P2 automatically gets the other class'),
        composites=['composite_menu.png', 'composite_select.png'],
        timing=dict(measured='base replay: START@700 -> select screen fading in ~frame 800, menu window ~870, B@900 -> ONE PLAYER., B@1000 -> fade -> name entry ~1030'),
        next='name entry (same routine)')

# ---------------------------------------------------------------- Name entry (0x6740.., main state 2 continued)
def nameentry():
    d = odir('nameentry')
    vm = VRAM()
    vm.put(0x0000, lz(0x778A, 0x1208)[:0x2000]); font_vram(vm)
    pal = words(ROM, 0x7266, 64)
    vm.put_text(0xC216, ROM[0x75B0:0x75C0], 0x250)                 # ENTRY YOUR NAME!
    vm.put_nt(0xC598, words(ROM, 0x75C0, 15*13), 15, 13)            # letter grid box
    vm.put_nt(0xC30E, words(ROM, 0x7746, 7*4), 7, 4)                # NAME box P1
    vm.put_nt(0xE412, words(ROM, 0x777E, 3), 3, 1)                  # red squares P1 (plane B)
    a1 = render_plane(vm, PLANE_A, pal); b1 = render_plane(vm, PLANE_B, pal)
    save(a1, d, 'plane_a_1p.png'); save(b1, d, 'plane_b_1p.png')
    vm.put_nt(0xC332, words(ROM, 0x7746, 7*4), 7, 4)                # NAME box P2
    vm.put_nt(0xE436, words(ROM, 0x7784, 3), 3, 1)                  # P2 squares
    a2 = render_plane(vm, PLANE_A, pal); b2 = render_plane(vm, PLANE_B, pal)
    save(a2, d, 'plane_a_2p.png'); save(b2, d, 'plane_b_2p.png')
    save(render_rect(vm, PLANE_A, pal, 12, 11, 15, 13), d, 'grid_box.png')
    save(render_rect(vm, PLANE_A, pal, 7, 6, 7, 4), d, 'name_box.png')
    sq = render_rect(vm, PLANE_B, pal, 9, 8, 3, 1); save(sq, d, 'name_squares.png')
    curs = {}
    for nm, m in (('cursor_p1', 0x71F2), ('cursor_p2', 0x71FC)):
        img = Image.new('RGBA', (8, 8), (0, 0, 0, 0)); pc = mapping_pieces(m)[0]
        draw_tile(img, vm.m, pc['attr'] & 0x7FF, 0, 0, pal_rgb(pal)[48:64], bool(pc['attr'] & 0x800))
        save(img, d, nm + '.png'); curs[nm] = dict(file=nm + '.png', mapping=m, attr='0x%04X' % pc['attr'], dy=pc['dy'])
    comp = composite([b1, a1]); cur = Image.open(os.path.join(d, 'cursor_p1.png'))
    comp.alpha_composite(cur, (0xE8-0x80, 0xE0-0x80-1)); save(comp, d, 'composite_1p.png')
    spl = Image.new('RGBA', (320, 224), (0, 0, 0, 0)); spl.alpha_composite(cur, (0xE8-0x80, 0xE0-0x80-1)); save(spl, d, 'sprites_1p.png')
    grid = []; a = 0x720F
    for r in range(8):
        grid.append(' '.join('%02X' % x for x in ROM[a + r*8: a + r*8 + 8]))
    letters = [[chr(ROM[0x720F + 8*r + 1 + c]) for c in range(6)] for r in range(5)]
    return dict(
        code='0x6740..0x6950 (per-player handler 0x6978), part of 0x6340',
        tiles='same VRAM as character select (0x778A -> VRAM 0, font -> 0x5000)',
        palette=dict(rom=0x7266, fade_in='0x293C after layout', fade_out='0x2990 when all players done'),
        plane_a=dict(clear='0x2DE0: 40x28 cells of 0 at 0xC000',
                     title=dict(rom=0x75B0, text='ENTRY YOUR NAME!', vram=0xC216, **nt_pos(0xC216, PLANE_A), writer='0x2C40, cell = byte + 0x250'),
                     grid_box=dict(rom=0x75C0, cells='15x13', vram=0xC598, **nt_pos(0xC598, PLANE_A), file='grid_box.png'),
                     name_box_p1=dict(rom=0x7746, cells='7x4', vram=0xC30E, **nt_pos(0xC30E, PLANE_A), file='name_box.png'),
                     name_box_p2=dict(rom=0x7746, vram=0xC332, **nt_pos(0xC332, PLANE_A), only_if='2 players'),
                     files={'1p': 'plane_a_1p.png', '2p': 'plane_a_2p.png'}),
        plane_b=dict(name_squares_p1=dict(rom=0x777E, cells='3x1', vram=0xE412, **nt_pos(0xE412, PLANE_B)),
                     name_squares_p2=dict(rom=0x7784, vram=0xE436, **nt_pos(0xE436, PLANE_B)),
                     file='name_squares.png', files={'1p': 'plane_b_1p.png', '2p': 'plane_b_2p.png'},
                     note='red empty-slot squares sit on plane B under the typed letters (plane A, cell byte+0x250 at 0xC412 / 0xC436, row 8 col 9 / 27)'),
        cursors=dict(**curs, object_p1='0xFF0DC0', object_p2='0xFF0F40',
                     start_vdp={'p1': [0xE8, 0xE0], 'p2': [0x138, 0xE0]},
                     start_screen={'p1': [0x68, 0x5F], 'p2': [0xB8, 0x5F]},
                     note='P1 starts on A (grid col 0), P2 starts on D (col 3) - both use the same grid; P2 arrow is tile 0xBA h-flipped'),
        grid=dict(rom=0x720F, rows_hex=grid, letters=letters, end_row='row 5: FA FA FA FA 00(END) FB  -> only END selectable',
                  cell_px=16, origin_screen=[104, 96],
                  codes={'FD': 'left of col 0 -> wrap to right (x += 0x60, ptr += 6)', 'FC': 'right of col 5 -> wrap left (x -= 0x60)',
                         'FE': 'above row 0 -> wrap to bottom (y += 0x60)', 'FF': 'below END row -> wrap to top (y -= 0x60)',
                         'FA': 'skip right (in END row)', 'FB': 'skip left', '00': 'END',
                         'chars': ': = space(blank black cell), < = ".", = = "!"'},
                  end_pos_screen=[0x128-0x80, 0x130-0x80]),
        input=dict(dirs='new-press U/D/L/R move 16 px (sound 0xB4)', b='B: type letter at cursor (max 3; after the 3rd, cursor jumps to END (0x128,0x130)); on END: finish',
                   a_or_c='A or C: delete last letter (cell cleared to 0, name byte = 0x3A)',
                   end_rules='empty name -> default "FIG" (fighter) / "MAG" (mage); in 2P mode identical names are rejected (treated as delete)',
                   name_ram='P1 0xFF0A4E..50, P2 0xFF0A51..53 (initialised to 0x3A)', done='0xFF0D46 counts finished players; when == players -> fade 0x2990'),
        composite='composite_1p.png',
        next='main state 3 (game: castle throne room, room 0)')

# ---------------------------------------------------------------- Continue / Game over (in-game overlay, 0x2BFA4)
def continue_gameover():
    d = odir('continue_gameover')
    vm = VRAM(); font_vram(vm)
    pal = [0]*64
    pal[16+14] = 0x000; pal[16+15] = 0xEEE      # font uses colours 14 (black cell) / 15 (white) of line 1
    pal[14] = 0x000; pal[15] = 0xEEE
    pcs = mapping_pieces(0x2C0C2)
    img = render_pieces(vm, pcs, pal, 0, 0, Image.new('RGBA', (80, 8), (0, 0, 0, 0)))
    save(img, d, 'continue.png')
    # "CREDIT " shown while any player is dead (0x2BEFE, mapping 0x2BF6A at VDP (0x170,0x138); digit at x 0x1A8)
    save(render_pieces(vm, mapping_pieces(0x2BF6A), pal, 0, 0, Image.new('RGBA', (56, 8), (0, 0, 0, 0))), d, 'credit.png')
    digits = []
    for n in range(10):
        im = Image.new('RGBA', (8, 8), (0, 0, 0, 0)); draw_tile(im, vm.m, 0x280 + n, 0, 0, pal_rgb(pal)[16:32])
        fn = 'digit_%d.png' % n; save(im, d, fn); digits.append(fn)
    # GAME OVER typing animation 0x2C114
    a = 0x2C114; last = be16(ROM, a); frames = []
    for i in range(last + 1):
        dur = be16(ROM, a + 4 + i*6); m = be32(ROM, a + 6 + i*6)
        im = render_pieces(vm, mapping_pieces(m), pal, 0, 0, Image.new('RGBA', (72, 8), (0, 0, 0, 0)))
        fn = 'gameover_%d.png' % i; save(im, d, fn); frames.append(dict(file=fn, frames=dur, mapping=hex(m)))
    # preview over black
    pv = Image.new('RGBA', (320, 224), (0, 0, 0, 255)); pv.alpha_composite(img, (0xF0-0x80, 0xD8-0x80))
    pv.alpha_composite(Image.open(os.path.join(d, 'digit_9.png')), (0x140-0x80, 0xD8-0x80)); save(pv, d, 'preview_continue.png')
    pv = Image.new('RGBA', (320, 224), (0, 0, 0, 255)); pv.alpha_composite(Image.open(os.path.join(d, frames[-1]['file'])), (0xF8-0x80, 0xD8-0x80))
    save(pv, d, 'preview_gameover.png')
    return dict(
        code='0x2BFA4 (called every game frame from main loop 0x4150); revive 0x9E2A (per player); game-over end 0x4196/0x41DC',
        overlay='drawn as sprites on top of the running game screen (no separate screen); palette line 1 colours 14/15 (black cell / white glyph)',
        trigger='all players dead: word 0xFF0A54 (dead count) == players (0xFF0A40+1); skipped when cheat 0xFF0206 set',
        credits=dict(ram='0xFF0A7A', initial={'1 player': 2, '2 players': 4}, set_at=0x4592,
                     hud_digit='sprite tile 0x2280+credits at screen (296,184) drawn by 0x2BF46 (HUD credit counter)'),
        continue_=dict(condition='credits > 0', mapping=0x2C0C2, text='CONTINUE? (10 pieces: C O N T I N U E ? + black cell)', file='continue.png',
                       pos_vdp=[0xF0, 0xD8], pos_screen=[0x70, 0x58],
                       countdown=dict(ram='0xFF0BEA (starts 10), sub-counter 0xFF0BEC', digit='tile 0x2280 + (count-1): shows 9..0',
                                      pos_screen=[0xC0, 0x58], frames_per_digit=60, files=digits,
                                      on_zero='game over'),
                       input='START (player +0x6D bit7 new press) on a dead player: credits -= 1, player revived at partner/room spawn, HP/MP refilled (0x9E2A)'),
        game_over=dict(condition='credits == 0 or countdown expired -> 0xFF0BF0 = -1', anim=0x2C114, pos_vdp=[0xF8, 0xD8], pos_screen=[0x78, 0x58],
                       frames=frames, sound=0x96,
                       note='types G,A,M,E,space+O,V,E,R one step every 4 frames (8 steps); when the anim reaches its last frame 0xFF0A7C = -1; main loop then counts 0xFF0A5C to 300 frames, fades out (0x2990) and sets 0xFF0A72 = 1 -> main state 3 jumps to 0x20A (reset -> SEGA logo)',
                       hud='HUD message "   GAME OVER" (HUD id, see ui_rpg.md) also appears in the dead player HUD slot'),
        previews=['preview_continue.png', 'preview_gameover.png'])

# ---------------------------------------------------------------- Ending / staff roll (0x1135C, main state 4)
def ending():
    d = odir('ending')
    vm = VRAM(); font_vram(vm)
    vm.put(0x4000, lz(0x3CF94, 0x13AE)[0x1000:0x2000])
    vm.put(0x0000, lz(0x65AE6, 0xCA2)[:0x2000])
    vm.put(0xE000, lz(0x608C8, 0x46A)[:0x1000])
    vm.put_nt(0xC39A, words(ROM, 0x118AA, 13*13), 13, 13)
    pal = words(ROM, 0x1180A, 64)
    a = render_plane(vm, PLANE_A, pal); b = render_plane(vm, PLANE_B, pal)
    save(a, d, 'plane_a.png'); save(b, d, 'plane_b.png')
    save(composite([b, a]), d, 'composite_1.png')
    pr = render_rect(vm, PLANE_A, pal, 13, 7, 13, 13); save(pr, d, 'princess.png')
    pal2 = list(pal); pal2[16:32] = words(ROM, 0x1188A, 16)
    pr2 = render_rect(vm, PLANE_A, pal2, 13, 7, 13, 13); save(pr2, d, 'princess_tinted.png')
    # message window (dialogue id 0x47, shape 1)
    sh1 = window_shape(1)
    vm_w = VRAM(); vm_w.m[:] = vm.m
    msg = render_window(vm_w, pal, *sh1['cells']); save(msg, d, 'message_window.png')
    c = composite([b, a]); c.alpha_composite(msg, (sh1['x'], sh1['y'])); save(c, d, 'composite_message.png')
    # staff roll text lines
    lines = []; o = 0x11B0C
    while ROM[o] != 0xFF:
        lines.append(ROM[o:o+25]); o += 25
    W = 25*8; roll = Image.new('RGBA', (W, len(lines)*8), (0, 0, 0, 0))
    tmp = VRAM(); tmp.m[:] = vm.m
    for i, ln in enumerate(lines):
        tmp.put_text(0xE000, ln, 0x60)
        roll.alpha_composite(render_rect(tmp, PLANE_B, pal, 0, 0, 25, 1), (0, i*8))
    save(roll, d, 'staff_roll.png')
    glyph = Image.new('RGBA', (8*48, 8), (0, 0, 0, 0))
    for i in range(48): draw_tile(glyph, vm.m, 0x90 + i, i*8, 0, pal_rgb(pal)[0:16])
    save(glyph, d, 'staff_font_0x30_0x5F.png')
    # staff roll preview frame: princess slid to x=0 with tinted palette, masked rows
    pv = Image.new('RGBA', (320, 224), (0, 0, 0, 255)); pv.alpha_composite(pr2, (0, 56))
    pv.alpha_composite(roll.crop((0, 0, W, 15*8)), (112, 48)); save(pv, d, 'preview_staff.png')
    # final PRESENTED BY TAITO screen
    vf = VRAM(); fb = lz(0x639C4, 0xF8C); vf.put(0x2000, fb[:0x2000])
    vf.put_nt(0xC61A, words(ROM, 0x119FC, 12*6), 12, 6)
    palf = words(ROM, 0x11A8C, 64)
    fa = render_plane(vf, PLANE_A, palf); save(fa, d, 'final_plane_a.png'); save(composite([fa]), d, 'final_composite.png')
    save(render_rect(vf, PLANE_A, palf, 13, 12, 12, 6), d, 'final_presented_by_taito.png')
    text = [ln.decode('latin1').replace(':', ' ').replace('<', '.') for ln in lines]
    return dict(
        code='0x1135C (main state 4, entered when the ending room 0x2F playback finishes: 0x40E4 sets 0xFF0A72 = 2)',
        tiles=[dict(rom=0x639C4, lzss_len=0xF8C, part='bank tiles 0xD0..0xFF', vram=0x5000, what='font (window text)'),
               dict(rom=0x3CF94, lzss_len=0x13AE, part='bank tiles 0x80..0xFF', vram=0x4000, tiles='0x200-0x27F'),
               dict(rom=0x65AE6, lzss_len=0xCA2, vram=0x0000, tiles='0x000-0x0FF', what='princess portrait + forest/sky + staff font (tiles 0x90..0xBF, cell = byte + 0x60)')],
        plane_b=dict(rom=0x608C8, lzss_len=0x46A, cells='64x32', vram=0xE000, palette_line=3, file='plane_b.png', what='sky + forest background'),
        plane_a=dict(rom=0x118AA, format='raw words', cells='13x13', vram=0xC39A, **nt_pos(0xC39A, PLANE_A), palette_line=1, file='plane_a.png',
                     crop='princess.png'),
        palette=dict(rom=0x1180A, words=64, fade_in='0x293C'),
        sequence=[
            dict(step='fade in background + princess', frames='~42 (0x293C)'),
            dict(step='wait', frames=61),
            dict(step='dialogue id 0x47 "YOU HAVE DONE / A GREAT THING. / THANK YOU!" in window shape 1', window=sh1, file='message_window.png',
                 note='typewriter 1 glyph / 2 frames (room 0x2F rule), auto-close 0x51 frames after end (no button)'),
            dict(step='wait', frames=61),
            dict(step='music', sound=0x8E),
            dict(step='fade out palette line 3 only (background) - 0x2A30 with mask 8, one step / 2 frames'),
            dict(step='replace palette line 1 (princess) with 0x1188A (16 words)', file='princess_tinted.png'),
            dict(step='mask: plane A rows 0-5 and 21-31 filled with 0x828A (high-priority black font cell) -> staff visible only in y 48..167'),
            dict(step='slide princess left: hscroll A (0xFF0640) -= 2 every 2 frames, 52 times (104 px, ends at x=0)', frames=104),
            dict(step='clear plane B'),
            dict(step='staff roll', text_rom=0x11B0C, row_bytes=25, rows=len(lines), writer=0x117AC,
                 layout='each row written to plane B at col 14 (x=112), row (21 + n) mod 32, cell = byte + 0x60, palette 0',
                 scroll='vscroll B (0xFF05F2) += 0x3000/65536 px every 2 frames (0.09375 px/frame); new row every 8 px (~85.3 frames)',
                 total_frames=round(len(lines)*8/0.09375), file='staff_roll.png', font='staff_font_0x30_0x5F.png', text=text),
            dict(step='fade out (0x2990), clear VRAM'),
            dict(step='final screen PRESENTED BY + TAITO logo', tiles='whole font bank 0x639C4 -> VRAM 0x2000 (tiles 0x100..0x1FF)',
                 nametable=dict(rom=0x119FC, cells='12x6', vram=0xC61A, **nt_pos(0xC61A, PLANE_A)), palette=0x11A8C, sound=0x96,
                 hold='0x4B0 x 2 frames = 2400 frames or START (either pad)', file='final_composite.png'),
            dict(step='fade out, return -> jmp 0x20A (reset: SEGA logo)')],
        previews=['composite_1.png', 'composite_message.png', 'preview_staff.png', 'final_composite.png'])

# ---------------------------------------------------------------- Attract demo (main state 1, 0x3C2C)
def demo():
    inp = []; a = 0x4F2A; total = 0
    while be16(ROM, a) != 0:
        cnt, btn = ROM[a], ROM[a+1]; inp.append([cnt, btn]); total += cnt; a += 2
        if len(inp) > 5000: break
    return dict(
        code='0x3C2C (main state 1) -> 0x3D80 game init with 0xFF0238 = 2 (demo flag)',
        setup=dict(table=0x47F0, entry0=dict(room=1, entrance_table=0x1388A, entrance=1), players='2 players FIG + MAG (0x3C6C.. names, 0xFF0A40=1, classes 0/1)',
                   levels='0xFF0E4A = 0x32, 0xFF0FCA = 0x46 (0x46F0)', note='demo index 0xFF0204 wraps at 1 so only demo 0 (0x3D12: inputs 0x4F2A / 0x592A for P1/P2) is used'),
        input_format='pairs {u8 frames, u8 pad bits} read by 0xC06 (pad bits: 0 up,1 down,2 left,3 right,4 B,5 C,6 A,7 START); list ends at a 0x0000 word',
        p1_inputs_rom=0x4F2A, p2_inputs_rom=0x592A, p1_input_frames_until_0=total,
        exit=dict(start='START on either pad (not in room 0x2F) -> 0xFF0A72 = 4 -> main state 0 (title)',
                  end='input data exhausted -> 0xFF0A72 = 3 -> reset (0x20A) -> SEGA logo'),
        measured='no-input boot: demo state set at frame 3338, gameplay visible ~3450 (cave room, FIG/MAG LV 2)',
        recorder='0xC56 (0xFF0238 == 1) is a leftover input recorder (counts to 0x960 frames then resets)')

FLOW = dict(
    reset='0x20A -> 0x304 init -> 0x1D86 SEGA logo -> 0x3BE2 main state machine (word 0xFF0A42, jump table 0x3C06)',
    states={'0': 'title 0x10AF8 (after sound 0x81). START -> 2, timeout -> 1',
            '1': 'attract demo 0x3C2C (game with 0xFF0238 = 2). START -> 0 (title); inputs exhausted -> reset (SEGA logo)',
            '2': 'character select + name entry 0x6340 -> 3',
            '3': 'game 0x3D2C/0x3D80. 0xFF0A72: 1 = game over -> reset (SEGA logo); 2 = ending finished room 0x2F -> 4',
            '4': 'ending / staff roll 0x1135C -> reset'},
    fades=dict(fade_out='0x2990: 0x2A30 step every 2 frames (wait d1=1 -> 2 vblanks) until all 4 lines black; each step decrements ONE component (B, G, R in rotation via 0xFF022E, masks 0xE00/0x0E0/0x00E, delta 0x200/0x020/0x002) of every colour -> <= 21 steps = 42 frames',
               fade_in='0x293C(a0=palette): copy 64 words to target 0xFF02F0, clear current, then 0x29C4 steps every 2 frames (same component rotation, towards target) -> <= 42 frames',
               instant='0x28D6 copies target -> current', buffers='current 0xFF0270, target 0xFF02F0, dirty flag 0xFF0224',
               frame_wait='0x2B12: d1+1 vblanks, then input poll 0x4950'),
    debug_menu='0x6C18 "GAME MODE SELECT" (1P/2P GAME, PLAYER SELECT FIGHTER/MAGICIAN, STAGE SELECT, SOUND CHECK; strings 0x7182..0x71F1) - no references found, unused')

if __name__ == '__main__':
    meta = {}
    which = sys.argv[1:] or ['sega', 'title', 'charselect', 'nameentry', 'continue_gameover', 'ending', 'demo']
    for w in which:
        if w in globals():
            meta[w] = globals()[w](); print('built', w)
    old = {}
    p = os.path.join(OUT, 'screens.json')
    if os.path.exists(p): old = json.load(open(p))
    old.update(meta); old['flow'] = FLOW
    json.dump(old, open(p, 'w'), indent=1)
