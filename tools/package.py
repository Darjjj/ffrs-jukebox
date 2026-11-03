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
import os
import re
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WIN64 = 'FFRS/Binaries/Win64'
SCRIPTS = WIN64 + '/scripts'


def version():
    toml = open(os.path.join(ROOT, 'Cargo.toml'), encoding='utf-8').read()
    return re.search(r'^version = "([^"]+)"', toml, re.M).group(1)


def main():
    files = [
        ('vendor/ultimate-asi-loader/winmm.dll', WIN64 + '/winmm.dll'),
        ('vendor/ultimate-asi-loader/LICENSE', WIN64 + '/Ultimate-ASI-Loader_LICENSE.txt'),
        ('target/release/ffrs_jukebox.dll', SCRIPTS + '/ffrs_jukebox.asi'),
    ]
    # The build mustn't carry anyone's user folder in its debug paths.
    dll = open(os.path.join(ROOT, 'target/release/ffrs_jukebox.dll'), 'rb').read()
    home = os.path.expanduser('~').encode()
    assert home not in dll, 'build has a user path in it: rebuild with --remap-path-prefix'
    out = os.path.join(ROOT, 'release', f'FFRS-Jukebox-{version()}.zip')
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        for src, dst in files:
            z.write(os.path.join(ROOT, src), dst)
    print(out)
    for name in zipfile.ZipFile(out).namelist():
        print('  ' + name)


if __name__ == '__main__':
    main()
