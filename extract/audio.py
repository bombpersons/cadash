"""Render Cadash's music and sound effects to OGG files for the remake.

Usage (needs the `mdx` binary and the Python `soundfile` + `numpy` packages):
    MDX=/path/to/mdx python3 extract/audio.py

How it works:
* The 68k queues sound IDs with PlaySound (0x2642, d0 = id); the VBlank handler hands them to the Z80
  SMPS driver (ROM 0xC82) through Z80 RAM 0x1C0A. IDs 0x81..0x97 are music, 0x98..0xBA sound effects.
* For every ID we patch a copy of the ROM so the title screen (0x3C1A) plays that ID instead of 0x81 and then
  just keeps requesting VBlanks (the VBlank handler flushes the queue), and let mdx synthesise the audio.
* Music timing comes from the sequence data (extract/smps.py): every song sets YM2612 Timer A once, one
  sequence tick per Timer A overflow, so intro/loop lengths are exact. Each looping song is written as
  intro + one loop body; `loop_start` (in samples) tells the game where to jump back to. The FM phase is
  not bit-identical between iterations, so the last few ms of the loop body are cross-faded into what
  precedes the loop start.

Outputs game/assets/audio/{music,sfx}/XX.ogg and game/assets/audio/sounds.json.
"""
import json, os, struct, subprocess, sys, tempfile, wave
from concurrent.futures import ThreadPoolExecutor

import numpy as np
import soundfile as sf

sys.path.insert(0, os.path.dirname(__file__))
import smps

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..'))
ROM = os.path.join(ROOT, 'Cadash (USA, Korea) (En).md')
MDX = os.environ.get('MDX', 'mdx')
WORK = os.path.join(ROOT, 'work', 'audio', 'extract')
OUT = os.path.join(ROOT, 'game', 'assets', 'audio')
SR = 44100
START_FRAME = 330          # a little before the title screen calls PlaySound
CHUNK = 3600               # mdx caps a capture at 3600 frames; longer captures chain save states
TIMER_A_TICK = 144 / (53693175 / 7)   # seconds per Timer A count (YM2612 clock = MCLK / 7)
XFADE = 1024               # loop seam cross-fade, samples
TARGET_PEAK = 30000        # all files share one gain so music/SFX balance stays as on the cartridge

MUSIC = range(0x81, 0x98)
SFX = range(0x98, 0xBB)

NAMES = {   # from where the game plays them (room table 0x49DA, PlaySound callers)
    0x81: 'title', 0x82: 'character_select', 0x83: 'castle_village', 0x84: 'dark_forest', 0x85: 'room9',
    0x86: 'graveyard', 0x87: 'earth_cave', 0x88: 'red_cave', 0x89: 'forest', 0x8A: 'dark_cavern',
    0x8B: 'boss', 0x8C: 'final_castle', 0x8D: 'music_8d', 0x8E: 'ending', 0x8F: 'music_8f', 0x90: 'item_get',
    0x91: 'music_91', 0x92: 'level_up', 0x93: 'music_93', 0x94: 'music_94', 0x95: 'blue_cave_swamp',
    0x96: 'game_over', 0x97: 'game_start',
}


STUB = 0x7FF00   # free (0xFF-filled) ROM space for the test driver


def patched_rom(sid):
    """Copy of the ROM whose title screen (0x3C1A) plays `sid` and then idles.

    Music is started directly. A sound effect is played after the title music (0x81) has run for half a
    second and been stopped with 0xE1: the title song is what programs YM2612 Timer B (1C06 = 230), the
    clock the driver runs sound effects on, and it is always played before the game proper."""
    r = bytearray(open(ROM, 'rb').read())
    r[0x358:0x35C] = bytes.fromhex('4e714e71')            # skip the checksum test
    first = sid if sid in MUSIC else 0x81
    r[0x3C1C:0x3C1E] = struct.pack('>H', first)            # title: PlaySound(first)
    r[0x3C24:0x3C2A] = bytes.fromhex('4ef9') + struct.pack('>I', STUB)   # jmp STUB
    wait = STUB + 0x40                                   # waitvb: request VBlank mode 1 and wait
    code = bytes.fromhex('46fc2500')                     # move.w #$2500,sr
    if sid not in MUSIC:
        code += bytes.fromhex('743b')                    # moveq #59,d2
        code += bytes.fromhex('4eb9') + struct.pack('>I', wait) + bytes.fromhex('51cafff8')  # jsr waitvb; dbf d2,*-6
        code += bytes.fromhex('303c00e1') + bytes.fromhex('4eb900002642')       # PlaySound(0xE1)
        code += bytes.fromhex('7403')                    # moveq #3,d2
        code += bytes.fromhex('4eb9') + struct.pack('>I', wait) + bytes.fromhex('51cafff8')
        code += bytes.fromhex('303c') + struct.pack('>H', sid) + bytes.fromhex('4eb900002642')
    code += bytes.fromhex('4eb9') + struct.pack('>I', wait) + bytes.fromhex('60f8')     # forever: jsr waitvb
    assert len(code) <= 0x40
    r[STUB:STUB + len(code)] = code
    vb = bytes.fromhex('13fc000100ff0218' '4a3900ff0218' '66f8' '4e75')
    r[wait:wait + len(vb)] = vb
    assert len(r) == 0x80000
    p = os.path.join(WORK, 'roms', '%02x.md' % sid)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, 'wb').write(r)
    return p


def mdx_audio(rom, frames, start=0, state=None, save=False):
    cmd = [MDX, '--root', ROOT, 'audio', '--rom', rom, '--frames', str(frames), '--start-frame', str(start),
           '--instructions', '100000000', '--out', os.path.join(WORK, 'wav')]
    if state:
        cmd += ['--from-state', state]
    if save:
        cmd += ['--save-state']
    j = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)
    d = j.get('data')
    if not d or not d.get('frame_target_reached'):
        raise RuntimeError('mdx audio failed: %s' % json.dumps(j)[:1000])
    w = wave.open(d['wav_artifact']['path'])
    a = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).reshape(-1, 2)
    return a, (d['state_artifact']['id'] if d.get('state_artifact') else None)


def capture(sid, seconds):
    rom = patched_rom(sid)
    chunks = max(1, int(np.ceil(seconds * 60 / CHUNK)))
    parts, state = [], None
    for i in range(chunks):
        frames = CHUNK if chunks > 1 else int(seconds * 60) + 1
        a, state = mdx_audio(rom, frames, START_FRAME if i == 0 else 0, state, i < chunks - 1)
        parts.append(a)
    return np.concatenate(parts).astype(np.float64)


def onset(a, thr=8):
    nz = np.nonzero(np.abs(a).max(1) > thr)[0]
    return int(nz[0]) if len(nz) else 0


def tail_end(a, start, thr=12, pad=0.05):
    nz = np.nonzero(np.abs(a[start:]).max(1) > thr)[0]
    return start + (int(nz[-1]) + int(pad * SR) if len(nz) else 0)


def song_timing(sid):
    s = smps.song(sid)
    timer = None
    for t in s['tracks'].values():
        for ev in t['tempo_changes']:
            if ev[1] == 'EB':
                lo, hi = ev[2][0], ev[2][1]
                v = lo | (hi & 0x0F) << 8
                assert timer in (None, v), 'tempo change in %x' % sid
                timer = v
    tick = (1024 - timer) * TIMER_A_TICK
    tracks = s['tracks'].values()
    loops = [t for t in tracks if t['kind'] == 'loop']
    if loops:
        lens = {t['end'] - t['start'] for t in loops}
        assert len(lens) == 1, 'tracks loop with different lengths in %x: %s' % (sid, lens)
        return dict(tick=tick, loop_start=max(t['start'] for t in loops), loop_len=lens.pop())
    return dict(tick=tick, end=max(t['end'] for t in tracks))


def render_music(sid):
    tm = song_timing(sid)
    if 'loop_len' in tm:
        need = (tm['loop_start'] + tm['loop_len']) * tm['tick'] + 3
    else:
        need = tm['end'] * tm['tick'] + 6
    a = capture(sid, need)
    on = onset(a)
    if 'loop_len' in tm:
        s = on + int(round(tm['loop_start'] * tm['tick'] * SR))
        n = int(round(tm['loop_len'] * tm['tick'] * SR))
        # intro = [on, s + XFADE); loop body = [s + XFADE, s + n + XFADE) whose tail fades into a[s:s+XFADE]
        intro = a[on:s + XFADE]
        body = a[s + XFADE:s + n + XFADE].copy()
        r = np.linspace(0, 1, XFADE, endpoint=False)[:, None]
        body[-XFADE:] = body[-XFADE:] * (1 - r) + a[s:s + XFADE] * r
        return np.concatenate([intro, body]), len(intro)
    return a[on:tail_end(a, on)], None


def render_sfx(sid):
    a = capture(sid, 11)
    # skip the title music up to the silence after 0xE1, then start at the effect's first sample
    loud = np.abs(a).max(1) > 2
    run, gap = 0, None
    for i in range(onset(a) + int(SR * 0.9), len(a)):   # the title music plays for 60 frames
        run = 0 if loud[i] else run + 1
        if run == 1000:
            gap = i
            break
    on = gap + onset(a[gap:], 2)
    end = tail_end(a, on)
    return a[on:end], None


def main():
    os.makedirs(WORK, exist_ok=True)
    jobs = [(sid, render_music) for sid in MUSIC] + [(sid, render_sfx) for sid in SFX]
    with ThreadPoolExecutor(8) as ex:
        results = list(ex.map(lambda j: (j[0],) + j[1](j[0]), jobs))
    peak = max(np.abs(a).max() for _, a, _ in results if len(a))
    gain = TARGET_PEAK / peak
    print('gain %.2f' % gain)
    entries = []
    for sid, a, loop_start in results:
        kind = 'music' if sid in MUSIC else 'sfx'
        e = dict(id=sid, kind=kind)
        if kind == 'music':
            e['name'] = NAMES.get(sid, '')
        else:
            e['priority'] = smps.priority(sid)
            e['channels'] = smps.sfx_channels(sid)
        if len(a) == 0:
            e['file'] = None     # 0xBA: no audible output (silences the SFX channels)
        else:
            rel = '%s/%02x.ogg' % (kind, sid)
            path = os.path.join(OUT, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            pcm = np.clip(np.round(a * gain), -32768, 32767).astype(np.int16)
            with sf.SoundFile(path, 'w', SR, 2, format='OGG', subtype='VORBIS') as f:
                for i in range(0, len(pcm), 16384):     # one big write crashes libsndfile's Vorbis encoder
                    f.write(pcm[i:i + 16384])
            e['file'] = 'audio/' + rel
            e['seconds'] = round(len(a) / SR, 3)
        if loop_start is not None:
            e['loop_start'] = int(loop_start)
        entries.append(e)
        print('%02x %-5s %6.2fs %s' % (sid, kind, len(a) / SR, ('loop@%d' % loop_start) if loop_start else ''))
    json.dump(dict(sample_rate=SR, sounds=entries), open(os.path.join(OUT, 'sounds.json'), 'w'), indent=1)


if __name__ == '__main__':
    main()
