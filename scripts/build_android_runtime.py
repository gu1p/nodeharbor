#!/usr/bin/env python3
"""Build the pinned ARM64 Android runtime and its corresponding source bundle."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import tarfile
import urllib.request

from android_runtime import extract_source, verify_android_elf, verify_file

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with Path(path).open('rb') as source: return hashlib.file_digest(source, 'sha256').hexdigest()


def run(command, *, cwd=None, env=None):
    print('+ ' + shlex.join(map(str, command)), flush=True)
    subprocess.run(list(map(str, command)), cwd=cwd, env=env, check=True)


def download(spec, cache):
    name = spec['url'].rsplit('/', 1)[1]
    path = cache / name
    if not path.exists():
        # Reuse verified development downloads, without trusting the cache name.
        earlier = ROOT / '.local/android-runtime-sources' / name
        if earlier.is_file(): shutil.copyfile(earlier, path)
        else:
            partial = path.with_suffix(path.suffix + '.partial')
            try:
                with urllib.request.urlopen(spec['url'], timeout=120) as source, partial.open('wb') as output:
                    shutil.copyfileobj(source, output, 1024 * 1024)
                verify_file(partial, spec['sha256'])
                partial.replace(path)
            finally: partial.unlink(missing_ok=True)
    verify_file(path, spec['sha256'])
    return path


def cross_file(path, ndk, prefix, pkgconfig, extra=()):
    # JSON strings are valid Meson strings once quoted with repr. Only the NDK
    # target's pkg-config directory is visible; host GLib/libffi cannot leak in.
    bins = {'c': ndk / 'aarch64-linux-android33-clang', 'cpp': ndk / 'aarch64-linux-android33-clang++',
            'ar': ndk / 'llvm-ar', 'strip': ndk / 'llvm-strip', 'pkg-config': pkgconfig}
    lines = ['[binaries]'] + [f'{key} = {str(value)!r}' for key, value in bins.items()]
    lines += ["[host_machine]", "system = 'android'", "cpu_family = 'aarch64'", "cpu = 'aarch64'", "endian = 'little'",
              '[properties]', 'needs_exe_wrapper = true', f'pkg_config_libdir = {str(prefix / "lib/pkgconfig")!r}',
              '[built-in options]', f'c_args = {list(["-fPIC", *extra])!r}',
              "c_link_args = ['-Wl,-z,max-page-size=16384']", "default_library = 'static'"]
    path.write_text('\n'.join(lines) + '\n')


def source_archive(output, sources, identity, originals):
    def clean(entry):
        if '.git' in Path(entry.name).parts or '__pycache__' in Path(entry.name).parts: return None
        entry.uid = entry.gid = 0
        entry.uname = entry.gname = ''
        entry.mtime = 0
        return entry
    with tarfile.open(output, 'w:gz') as archive:
        archive.add(sources, arcname='sources', filter=clean)
        archive.add(originals, arcname='original-verified-archives', filter=clean)
        archive.add(ROOT / 'android/runtime', arcname='nodeharbor/android/runtime', filter=clean)
        for name in ['scripts/android_runtime.py', 'scripts/build_android_runtime.py']:
            archive.add(ROOT / name, arcname='nodeharbor/' + name, filter=clean)
        archive.add(identity, arcname='runtime-build.json', filter=clean)


def build(work, jobs, bundle_sources):
    lock_path = ROOT / 'android/runtime/lock.json'
    lock = json.loads(lock_path.read_text())
    host = {'Darwin': 'darwin-x86_64', 'Linux': 'linux-x86_64'}.get(platform.system())
    if host is None or (platform.system() == 'Linux' and platform.machine() != 'x86_64'):
        raise ValueError('Build the Android runtime on macOS or an x86_64 Linux host using the Android NDK')
    sdk = Path(os.environ.get('ANDROID_HOME') or os.environ.get('ANDROID_SDK_ROOT') or '')
    ndk = sdk / 'ndk' / lock['ndk'] / 'toolchains/llvm/prebuilt' / host / 'bin'
    if not (ndk / 'aarch64-linux-android33-clang').is_file(): raise ValueError('Install the pinned Android NDK ' + lock['ndk'])
    pkgconfig = shutil.which('pkg-config')
    if not pkgconfig: raise ValueError('Install pkg-config to build the Android runtime')
    if subprocess.check_output(['meson', '--version'], text=True).strip() != lock['meson']:
        raise ValueError('Install the pinned Meson version ' + lock['meson'])
    patch = ROOT / 'android/runtime/qemu-android.patch'
    tree = work / ('tree-' + digest(lock_path)[:12] + '-' + digest(patch)[:12])
    sources, prefix = tree / 'sources', tree / 'prefix'
    sources.mkdir(parents=True, exist_ok=True)
    cache = work / 'downloads'
    cache.mkdir(parents=True, exist_ok=True)
    for name in ['glib', 'qemu']:
        spec = lock[name]
        destination = sources / name
        if not destination.exists():
            # No firmware is built or installed for direct kernel boot. QEMU's
            # unused EDK2 host source contains an absolute X11 symlink; leave the
            # firmware tree in its original archive, never follow it on the host.
            excluded = (f'qemu-{spec["version"]}/roms',) if name == 'qemu' else ()
            extract_source(download(spec, cache), destination, exclude_prefixes=excluded)
    glib = sources / 'glib' / ('glib-' + lock['glib']['version'])
    qemu = sources / 'qemu' / ('qemu-' + lock['qemu']['version'])
    patched = tree / 'patched'
    if not patched.exists():
        run(['patch', '--batch', '-p1', '-i', patch], cwd=qemu)
        patched.write_text(digest(patch) + '\n')
    elif patched.read_text().strip() != digest(patch): raise ValueError('The cached runtime patch identity changed')
    slirp = sources / 'slirp'
    if not slirp.exists():
        run(['git', 'init', slirp])
        run(['git', '-C', slirp, 'fetch', '--depth=1', lock['slirp']['url'], lock['slirp']['commit']])
        run(['git', '-C', slirp, 'checkout', '--detach', 'FETCH_HEAD'])
    if subprocess.check_output(['git', '-C', str(slirp), 'rev-parse', 'HEAD'], text=True).strip() != lock['slirp']['commit']:
        raise ValueError('The cached libslirp source does not match its pin')
    cross = tree / 'android.cross'
    cross_file(cross, ndk, prefix, pkgconfig)
    environment = dict(os.environ, PKG_CONFIG_LIBDIR=str(prefix / 'lib/pkgconfig'), PKG_CONFIG_PATH='')
    glib_build = tree / 'glib'
    if not (glib_build / 'build.ninja').is_file():
        run(['meson', 'setup', glib_build, glib, '--cross-file', cross, '--prefix', prefix, '--libdir=lib',
             '--buildtype=release', '-Dtests=false', '-Dinstalled_tests=false', '-Dintrospection=disabled',
             '-Ddocumentation=false', '-Dman-pages=disabled', '-Dselinux=disabled', '-Dlibmount=disabled',
             '-Dlibelf=disabled', '-Dnls=disabled', '-Dsysprof=disabled', '-Ddtrace=disabled', '-Dsystemtap=disabled'], env=environment)
    run(['ninja', '-C', glib_build, '-j', jobs], env=environment)
    run(['meson', 'install', '-C', glib_build, '--no-rebuild'], env=environment)
    qemu_build = tree / 'qemu'
    qemu_build.mkdir(exist_ok=True)
    for key, tool in {'AR': 'ar', 'NM': 'nm', 'STRIP': 'strip', 'RANLIB': 'ranlib', 'READELF': 'readelf'}.items():
        environment[key] = str(ndk / ('llvm-' + tool))
    environment['PKG_CONFIG'] = pkgconfig
    if not (qemu_build / 'build.ninja').is_file():
        run([qemu / 'configure', '--cross-prefix=', '--target-list=aarch64-softmmu', '--cpu=aarch64',
             f'--cc={ndk}/aarch64-linux-android33-clang', f'--cxx={ndk}/aarch64-linux-android33-clang++',
             '--host-cc=cc', '--without-default-features', '--enable-system', '--enable-tcg', '--enable-fdt',
             '--disable-rust', '--disable-pie', '--extra-cflags=-fPIC', '--extra-ldflags=-Wl,-z,max-page-size=16384',
             '--disable-docs', '--disable-install-blobs', '--disable-containers', '--container-command=false'], cwd=qemu_build, env=environment)
    run(['ninja', '-C', qemu_build, '-j', jobs, 'libqemu-system-aarch64.so'], env=environment)
    slirp_cross = tree / 'slirp.cross'
    cross_file(slirp_cross, ndk, prefix, pkgconfig,
               ['-DNODEHARBOR_SLIRP', '-include', str(ROOT / 'android/runtime/network/socket_adapter.h')])
    slirp_build = tree / 'slirp'
    if not (slirp_build / 'build.ninja').is_file():
        run(['meson', 'setup', slirp_build, slirp, '--cross-file', slirp_cross, '--prefix', prefix,
             '--libdir=lib', '--buildtype=release'], env=environment)
    run(['ninja', '-C', slirp_build, '-j', jobs, 'libslirp.a'], env=environment)
    run(['meson', 'install', '-C', slirp_build, '--no-rebuild'], env=environment)
    destination = ROOT / 'android/app/src/main/jniLibs/arm64-v8a'
    destination.mkdir(parents=True, exist_ok=True)
    native = destination / 'libnodeharbor-network.so'
    flags = shlex.split(subprocess.check_output([pkgconfig, '--cflags', '--libs', '--static', 'slirp'], env=environment, text=True))
    run([ndk / 'aarch64-linux-android33-clang', '-shared', '-fPIC', '-std=c11', '-D_GNU_SOURCE', '-O2',
         '-Wall', '-Wextra', '-Werror', '-fvisibility=hidden', '-Wl,-z,max-page-size=16384', '-Wl,--exclude-libs,ALL',
         ROOT / 'android/runtime/network/socket_adapter.c', ROOT / 'android/runtime/network/packet_loop.c',
         *flags, '-o', native])
    shutil.copyfile(qemu_build / 'libqemu-system-aarch64.so', destination / 'libqemu-system-aarch64.so')
    libraries = {}
    for path in destination.glob('*.so'):
        verify_android_elf(path)
        libraries[path.name] = digest(path)
    identity = destination.parent / 'runtime-build.json'
    identity.write_text(json.dumps({'formatVersion': 1, 'abi': lock['abi'], 'lockSha256': digest(lock_path),
                                   'patchSha256': digest(patch), 'qemuVersion': lock['qemu']['version'],
                                   'networkSources': {p.name: digest(p) for p in (ROOT / 'android/runtime/network').iterdir() if p.is_file()},
                                   'libraries': libraries}, indent=2) + '\n')
    if bundle_sources:
        output = work / 'nodeharbor-android-runtime-source.tar.gz'
        source_archive(output, sources, identity, cache)
        print('Corresponding runtime sources: ' + str(output))
    print('Verified Android runtime: ' + str(identity))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, default=ROOT / '.local/android-native')
    parser.add_argument('--jobs', type=int, default=min(8, os.cpu_count() or 2))
    parser.add_argument('--bundle-sources', action='store_true')
    args = parser.parse_args()
    if not 1 <= args.jobs <= 64: parser.error('Use between 1 and 64 build jobs')
    build(args.work.resolve(), args.jobs, args.bundle_sources)


if __name__ == '__main__': main()
