#!/usr/bin/env python3
"""Dump all Cadash game text to extract/out/text/strings.json.

Encoding: one byte per glyph, glyph tile = byte - 0x30 within the font bank
(font = LZSS 0x639C4 tiles 0xD0..0xFF; VDP tile = byte + 0x250, palette 0, priority).
Control codes (dialogue engine 0x2C4B6, HUD-message engine 0x8C9A) are documented in CODES.
"""
import json, os, struct, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
rom = open(os.path.join(ROOT, 'Cadash (USA, Korea) (En).md'), 'rb').read()

CHARMAP = {0x3A: ' ', 0x3B: ',', 0x3C: '.', 0x3D: '!', 0x3E: "'", 0x3F: '?', 0x40: '@'}
for c in range(0x30, 0x3A): CHARMAP[c] = chr(c)
for c in range(0x41, 0x5B): CHARMAP[c] = chr(c)
# 0x5B '[' and 0x5C '\' are special glyphs in the font (see font.json)
CHARMAP[0x5B] = '[' ; CHARMAP[0x5C] = '\\'

def be32(o): return struct.unpack('>I', rom[o:o + 4])[0]

# control codes: (name, operand bytes, description)
CODES = {
    0xFF: ('END', 0, 'end of text: dialogue waits for B (auto 0x50 frames in ending room 0x2F / small window); HUD msg ends'),
    0xFE: ('CONT', 0, 'dialogue: fall through to control-code processing (no wait); HUD msg: wait 60 frames then end'),
    0xFD: ('NL', 0, 'new line (next tile row)'),
    0xFC: ('PAGE', 0, 'dialogue: show prompt glyph 0x40 and wait for B (auto 0x3C frames in room 0x2F), clear window; HUD: wait 60 frames, clear'),
    0xFB: ('IF_FLAG', 8, 'if byte[ptr1] != 0 goto ptr2'),
    0xFA: ('INC_FLAG', 4, 'byte[ptr] += 1'),
    0xF0: ('CLR_FLAG', 4, 'byte[ptr] = 0'),
    0xED: ('SET_FLAG', 5, 'byte[ptr] = imm8'),
    0xF9: ('IF_2P', 4, 'if byte 0xFF0A41 != 0 (second player present) goto ptr'),
    0xF6: ('GOTO', 4, 'goto ptr'),
    0xF8: ('P1_NAME', 0, 'insert 3-char name at 0xFF0A4E'),
    0xF7: ('P2_NAME', 0, 'insert 3-char name at 0xFF0A51'),
    0xF5: ('SPEAKER_NAME', 0, 'insert name of player index (0xFFE5D4+0x32) via table 0x2CA0E'),
    0xF4: ('PLAYER_NAME', 0, 'HUD msg: name of the player owning the message line'),
    0xEF: ('NUMBER', 0, 'insert 5-digit number (leading zeros as spaces) from word +0xD0 of object 0xFF0BB2 (price)'),
    0xEE: ('YESNO', 0, 'set 0xFF0BC2=1 and stop: open YES/NO choice'),
    0xF3: ('DIGIT', 0, 'HUD msg: one digit from 0xFF0A7A (clamped to 9)'),
    0xF2: ('IF_ZERO_A7A', 4, 'HUD msg: if word 0xFF0A7A == 0 goto ptr'),
    0xF1: ('IF_FF0206', 4, 'HUD msg: if word 0xFF0206 != 0 goto ptr'),
}
PRE = {0xFB, 0xFA, 0xF0, 0xED, 0xF9, 0xF6}


def parse(addr):
    """Parse a text node starting at addr. Returns (tokens, text, end_addr, jump targets)."""
    toks, text, jumps = [], '', []
    a = addr
    while a < len(rom):
        b = rom[a]
        if b in CODES:
            name, n, _ = CODES[b]
            ops = rom[a + 1:a + 1 + n]
            tok = {'op': name}
            if b == 0xFB:
                tok['flag'] = '0x%06X' % (be32(a + 1) & 0xFFFFFF); tok['target'] = '0x%05X' % be32(a + 5); jumps.append(be32(a + 5))
            elif b in (0xFA, 0xF0):
                tok['flag'] = '0x%06X' % (be32(a + 1) & 0xFFFFFF)
            elif b == 0xED:
                tok['flag'] = '0x%06X' % (be32(a + 1) & 0xFFFFFF); tok['value'] = rom[a + 5]
            elif b in (0xF9, 0xF6, 0xF2, 0xF1):
                tok['target'] = '0x%05X' % be32(a + 1); jumps.append(be32(a + 1))
            toks.append(tok)
            text += {'NL': '\n', 'PAGE': '{PAGE}\n', 'P1_NAME': '{P1}', 'P2_NAME': '{P2}', 'SPEAKER_NAME': '{NAME}',
                     'PLAYER_NAME': '{NAME}', 'NUMBER': '{NUM}', 'YESNO': '{YESNO}', 'DIGIT': '{D}'}.get(name, '' if name in ('END', 'CONT') else '{%s}' % name)
            a += 1 + n
            if b in (0xFF, 0xF6, 0xEE):
                break
            if b == 0xFE and rom[a] not in PRE and rom[a] != 0xFF:
                break  # HUD message terminator
            continue
        if b in CHARMAP:
            ch = CHARMAP[b]
            if toks and toks[-1].get('op') == 'TEXT':
                toks[-1]['s'] += ch
            else:
                toks.append({'op': 'TEXT', 's': ch})
            text += ch
            a += 1
            continue
        break  # unknown byte -> stop
    return toks, text, a, jumps


def main():
    out = {'encoding': {
        'note': 'glyph index = byte - 0x30 into font bank (LZSS 0x639C4 tiles 0xD0..0xFF); VDP tile word = 0x8250 + byte',
        'charmap': {'0x%02X' % k: v for k, v in sorted(CHARMAP.items())},
        'control_codes': {'0x%02X' % k: {'name': v[0], 'operand_bytes': v[1], 'desc': v[2]} for k, v in sorted(CODES.items())},
    }}
    # 1) dialogue / message table
    tbl = 0x2D528
    n = (0x2D78C - tbl) // 4
    entries = []
    nodes = {}
    pending = []
    for i in range(n):
        p = be32(tbl + 4 * i)
        entries.append({'id': i + 1, 'addr': '0x%05X' % p, 'hud_message': i + 1 >= (0x2D750 - tbl) // 4 + 1})
        pending.append(p)
    while pending:
        p = pending.pop()
        if p in nodes or not (0 < p < len(rom)): continue
        toks, text, end, jumps = parse(p)
        nodes[p] = {'addr': '0x%05X' % p, 'end': '0x%05X' % end, 'text': text, 'tokens': toks}
        pending += jumps
    for e in entries:
        e['text'] = nodes[int(e['addr'], 16)]['text']
    out['dialogue_table'] = {'addr': '0x2D528', 'count': n,
                             'note': 'dialogue id (1-based) in word 0xFFE5D4+2; HUD player messages use id = word player+0xB2 indexed from 0x2D750 (entry 139+id-1)',
                             'entries': entries}
    out['nodes'] = [nodes[k] for k in sorted(nodes)]

    # 2) fixed strings
    def raw(a, l):
        return ''.join(CHARMAP.get(c, '{%02X}' % c) for c in rom[a:a + l])
    out['item_names'] = {'addr': '0x2D4FE', 'ref': 'pointer at 0x2D324', 'format': '13-byte space-padded fields each followed by 0x5C (stride 14)',
                         'names': [raw(0x2D4FE + 14 * i, 13).rstrip() for i in range(3)]}
    fixed = [
        (0x2D4FC, 2, 'level label "LV"'),
        (0x2D194, 4, 'status label GOLD'), (0x2D198, 3, 'status label EXP'),
        (0x2D1A0, 3, 'status: STR'), (0x2D1A4, 4, 'status: A.C.'), (0x2D1A8, 3, 'status: AGL'),
        (0x33038, 2, 'LV label (shop/status)'), (0x3303A, 4, 'GOLD'), (0x3303F, 3, 'YES'), (0x33043, 2, 'NO'),
        (0x7246, 16, 'character select menu "1PLAYER 2PLAYERS"'),
        (0x75B0, 16, 'name entry title "ENTRY YOUR NAME!"'),
        (0x9048, 9, 'HUD "NO PLAYER"'),
    ]
    out['fixed_strings'] = [{'addr': '0x%05X' % a, 'len': l, 'text': raw(a, l), 'use': u} for a, l, u in fixed]
    # name entry grid (0x7216): FD row start, FC row end, FA = END slot, FB ...
    grid = []
    a = 0x720E
    row = ''
    while a < 0x7246:
        b = rom[a]
        if b == 0xFD: row = ''
        elif b == 0xFC: grid.append(row)
        elif b in CHARMAP: row += CHARMAP[b]
        a += 1
    out['name_entry_grid'] = {'addr': '0x720E', 'rows': grid, 'note': 'FD/FC delimit rows; trailing FA bytes = END cell'}
    # staff roll
    out['staff_roll'] = staff()
    os.makedirs(os.path.join(ROOT, 'extract/out/text'), exist_ok=True)
    json.dump(out, open(os.path.join(ROOT, 'extract/out/text/strings.json'), 'w'), indent=1)
    print('entries', n, 'nodes', len(nodes))


def staff():
    # staff roll (ending, code 0x11644..0x117AC): rows of 25 bytes written to plane B (VRAM 0xE000) column 14,
    # one new row each 8px of scroll; vscroll += 0x3000/0x10000 px per frame; terminated by 0xFF.
    a = 0x11B0C
    lines = []
    while rom[a] != 0xFF:
        lines.append(''.join(CHARMAP.get(c, '{%02X}' % c) for c in rom[a:a + 25]).rstrip())
        a += 25
    return {'addr': '0x11B0C', 'row_width': 25, 'plane_col': 14, 'scroll_px_per_frame': 0x3000 / 65536.0,
            'end': '0x%05X' % a, 'lines': lines}


if __name__ == '__main__':
    main()
