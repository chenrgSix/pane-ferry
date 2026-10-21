#!/usr/bin/env python3
"""Build an optional, pinned WebRTC runtime without changing system packages."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

REVISION = '8e330edebfe766b44f59be427e0454fd18964d4e'  # libdatachannel v0.24.4
ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/webrtc')
    parser.add_argument('--source', type=Path)
    parser.add_argument('--cmake', default='cmake')
    parser.add_argument('--openssl-root', type=Path)
    parser.add_argument('--jobs', type=int, default=4)
    args = parser.parse_args()
    assert 1 <= args.jobs <= 16
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = (args.source or output / 'libdatachannel').resolve()
    if not source.exists():
        subprocess.run(['git', 'clone', '--depth', '1', '--branch', 'v0.24.4',
                        '--recurse-submodules', '--shallow-submodules',
                        'https://github.com/paullouisageneau/libdatachannel.git', str(source)], check=True)
    if (source / '.git').exists():
        revision = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    else:
        revision = (source / '.remoteapp-source-revision').read_text().strip()
    assert revision == REVISION, 'libdatachannel source revision differs from the reviewed pin'
    build = output / 'build'
    command = [args.cmake, '-S', str(ROOT / 'native/webrtc'), '-B', str(build), '-G', 'Ninja',
               '-DCMAKE_BUILD_TYPE=Release', '-DREMOTEAPP_LIBDATACHANNEL_SOURCE=' + str(source)]
    openssl = args.openssl_root
    if openssl is None and sys.platform == 'darwin':
        openssl = Path('/opt/homebrew/opt/openssl@3')
    if openssl:
        openssl = openssl.resolve()
        command += ['-DOPENSSL_ROOT_DIR=' + str(openssl)]
        multiarch = openssl / 'include/x86_64-linux-gnu'
        if multiarch.exists():
            # Linux already supplies the OpenSSL runtime. Ubuntu's static
            # archive can depend on extra entropy-provider development libs.
            system = Path('/usr/lib/x86_64-linux-gnu')
            command += ['-DCMAKE_C_FLAGS=-isystem ' + str(multiarch),
                        '-DCMAKE_CXX_FLAGS=-isystem ' + str(multiarch),
                        '-DOPENSSL_USE_STATIC_LIBS=OFF',
                        '-DOPENSSL_INCLUDE_DIR=' + str(openssl / 'include'),
                        '-DOPENSSL_CRYPTO_LIBRARY=' + str(system / 'libcrypto.so.3'),
                        '-DOPENSSL_SSL_LIBRARY=' + str(system / 'libssl.so.3')]
    subprocess.run(command, check=True)
    subprocess.run([args.cmake, '--build', str(build), '--target', 'remoteapp_webrtc',
                    'remoteapp_webrtc_test', 'remoteapp_webrtc_receiver_test', '--parallel', str(args.jobs)], check=True)
    subprocess.run([str(build / 'remoteapp_webrtc_test')], check=True)
    subprocess.run([str(build / 'remoteapp_webrtc_receiver_test')], check=True)
    library = 'libremoteapp_webrtc.' + ('dylib' if sys.platform == 'darwin' else 'so')
    shutil.copy2(build / library, output / library)
    licenses = output / 'licenses'
    licenses.mkdir(exist_ok=True)
    for name, directory in [('libdatachannel', source), ('libjuice', source / 'deps/libjuice'),
                            ('libsrtp', source / 'deps/libsrtp'), ('usrsctp', source / 'deps/usrsctp'),
                            ('plog', source / 'deps/plog')]:
        for file in directory.iterdir():
            if file.is_file() and file.name.upper().startswith(('LICENSE', 'COPYING')):
                shutil.copy2(file, licenses / (name + '-' + file.name))
    if openssl:
        for path in [openssl / 'LICENSE.txt', openssl / 'share/doc/openssl/LICENSE.txt', openssl / 'share/doc/libssl-dev/copyright']:
            if path.is_file():
                shutil.copy2(path, licenses / 'OpenSSL-LICENSE.txt')
                break
    (output / 'libdatachannel-revision.txt').write_text(REVISION + '\n')
    shutil.copy2(ROOT / 'THIRD_PARTY_NOTICES.md', output / 'THIRD_PARTY_NOTICES.md')
    print('WebRTC runtime:', output / library)


if __name__ == '__main__':
    main()
