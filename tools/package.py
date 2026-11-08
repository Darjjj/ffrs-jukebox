"""Builds the release zip: one FFRS folder that drops straight into the game's
FINAL FANTASY RESONANCE folder, ASI loader included.

    python tools/package.py            (after a clean `cargo build --release`)

Layout:
    FFRS/Binaries/Win64/winmm.dll                          Ultimate ASI Loader
    FFRS/Binaries/Win64/Ultimate-ASI-Loader_LICENSE.txt    its MIT notice, which has to ship with it
    FFRS/Binaries/Win64/scripts/ffrs_jukebox.asi           the jukebox

The readme, changelog and the jukebox's own license live on GitHub and Nexus,
which keeps the scripts folder to the jukebox's own files.
"""
import glob
import os
import re
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WIN64 = 'FFRS/Binaries/Win64'
SCRIPTS = WIN64 + '/scripts'
DLL = 'target/release/ffrs_jukebox.dll'


def version():
    toml = open(os.path.join(ROOT, 'Cargo.toml'), encoding='utf-8').read()
    return re.search(r'^version = "([^"]+)"', toml, re.M).group(1)


def fail(why):
    sys.exit('package: ' + why)


def main():
    files = [
        ('vendor/ultimate-asi-loader/winmm.dll', WIN64 + '/winmm.dll'),
        ('vendor/ultimate-asi-loader/LICENSE', WIN64 + '/Ultimate-ASI-Loader_LICENSE.txt'),
        (DLL, SCRIPTS + '/ffrs_jukebox.asi'),
    ]
    for src, _ in files:
        if not os.path.isfile(os.path.join(ROOT, src)):
            fail(f'missing {src}')
    # The dll must be newer than every source and the version bump, or the
    # zip would carry an old build under a new number.
    built = os.path.getmtime(os.path.join(ROOT, DLL))
    sources = glob.glob(os.path.join(ROOT, 'src', '*.rs')) + [os.path.join(ROOT, 'Cargo.toml')]
    stale = [s for s in sources if os.path.getmtime(s) > built]
    if stale:
        fail('the build is older than ' + ', '.join(os.path.relpath(s, ROOT) for s in stale) + ': run cargo build --release first')
    # The build mustn't carry anyone's user folder in its debug paths.
    dll = open(os.path.join(ROOT, DLL), 'rb').read().lower()
    home = os.path.expanduser('~').encode().lower()
    if home in dll:
        fail('the build has a user path in it: rebuild with --remap-path-prefix (see the README)')
    out = os.path.join(ROOT, 'release', f'FFRS-Jukebox-{version()}.zip')
    os.makedirs(os.path.dirname(out), exist_ok=True)
    if os.path.exists(out):
        print(f'replacing {out}')
    part = out + '.part'
    with zipfile.ZipFile(part, 'w', zipfile.ZIP_DEFLATED) as z:
        for src, dst in files:
            z.write(os.path.join(ROOT, src), dst)
    os.replace(part, out)
    print(out)
    for name in zipfile.ZipFile(out).namelist():
        print('  ' + name)


if __name__ == '__main__':
    main()
