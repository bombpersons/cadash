#!/usr/bin/env python3
"""Extracts the game's assets from a Cadash cartridge image into assets/.

    python3 extract/extract_assets.py "Cadash (USA, Korea) (En).md" [--mdx PATH] [--no-audio] [--out DIR]

Needs Python 3 with Pillow and numpy; the audio step also needs `soundfile` and the mdx binary
(https://github.com/bombpersons/mdx), found via --mdx, $MDX or PATH. Only the USA/Korea release is accepted.

The scripts were written against a project layout of <root>/extract (scripts and their intermediate
`out/`), <root>/game/assets (output) and <root>/work (scratch). This driver recreates that layout under
target/extract-root and links game/assets to the output directory, so the scripts run unchanged.
"""
import argparse, hashlib, os, shutil, subprocess, sys

ROM_SHA256 = '85e0782411cf50ea92a9982dac1942679f738d46a58fdda5d7d82b943512afd8'
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

# (script, arguments). Intermediate passes write extract/out/, the rest write game/assets/.
STEPS = [
    ('text.py', []),
    ('ui_font.py', []),
    ('rpg_data.py', []),
    ('collision.py', []),
    ('rooms.py', []),
    ('screens.py', []),
    ('objects.py', []),
    ('boss_tables.py', []),
    ('build_assets.py', []),  # font, hud, text, screens, rooms (+ room events)
    ('player.py', []),
    ('objects_game.py', []),
    ('opcodes_game.py', []),
    ('gamedata.py', []),
    ('magic.py', []),
    ('weapon_shot.py', []),
    ('shops.py', []),
]
AUDIO = ('audio.py', [])


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('rom', help='Cadash (USA, Korea) cartridge image')
    ap.add_argument('--out', default=os.path.join(REPO, 'assets'), help='output directory (default: assets/)')
    ap.add_argument('--mdx', default=os.environ.get('MDX') or shutil.which('mdx'), help='mdx binary (audio)')
    ap.add_argument('--no-audio', action='store_true', help='skip rendering music and sound effects')
    a = ap.parse_args()

    rom = os.path.abspath(a.rom)
    digest = hashlib.sha256(open(rom, 'rb').read()).hexdigest()
    if digest != ROM_SHA256:
        sys.exit(f'{rom}: unexpected SHA-256 {digest}\nexpected the USA/Korea release ({ROM_SHA256})')
    if not a.no_audio and not a.mdx:
        sys.exit('audio needs the mdx binary: pass --mdx PATH or set $MDX (or use --no-audio)')

    root = os.path.join(REPO, 'target', 'extract-root')
    shutil.rmtree(root, ignore_errors=True)
    shutil.copytree(HERE, os.path.join(root, 'extract'), ignore=shutil.ignore_patterns('__pycache__', 'out'))
    os.makedirs(os.path.join(root, 'work'))
    os.makedirs(os.path.join(root, 'game'))
    out = os.path.abspath(a.out)
    os.makedirs(out, exist_ok=True)
    os.symlink(out, os.path.join(root, 'game', 'assets'))
    os.symlink(rom, os.path.join(root, 'Cadash (USA, Korea) (En).md'))

    env = dict(os.environ, CADASH_ROM=rom, PYTHONDONTWRITEBYTECODE='1')
    if a.mdx:
        env['MDX'] = os.path.abspath(a.mdx)
    steps = STEPS + ([] if a.no_audio else [AUDIO])
    for script, args in steps:
        print(f'== {script}', flush=True)
        subprocess.run([sys.executable, os.path.join(root, 'extract', script), *args], cwd=root, env=env, check=True,
                       stdout=subprocess.DEVNULL)
    print(f'assets written to {out}')


if __name__ == '__main__':
    main()
