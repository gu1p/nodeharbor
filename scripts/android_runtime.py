#!/usr/bin/env python3
"""Verified source handling and descriptor-only Android VM configuration."""
import hashlib
from pathlib import Path, PurePosixPath
import re
import tarfile
import tempfile
import struct


def verify_android_elf(path):
    """Reject wrong architectures and libraries incompatible with 16 KiB pages."""
    with Path(path).open('rb') as source:
        header = source.read(64)
        if len(header) != 64 or header[:7] != b'\x7fELF\x02\x01\x01':
            raise ValueError('Android requires an ELF64 little-endian library')
        kind, machine = struct.unpack_from('<HH', header, 16)
        if (kind, machine) != (3, 183): raise ValueError('Android requires ARM64 shared libraries')
        offset, = struct.unpack_from('<Q', header, 32)
        size, count = struct.unpack_from('<HH', header, 54)
        if size != 56 or not 1 <= count <= 128: raise ValueError('Invalid ELF program headers')
        source.seek(offset)
        loads = 0
        for _ in range(count):
            entry = source.read(size)
            if len(entry) != size: raise ValueError('Truncated ELF program headers')
            kind, _, file_offset, virtual, _, _, _, alignment = struct.unpack('<IIQQQQQQ', entry)
            if kind == 1:
                loads += 1
                if alignment < 16384 or alignment & (alignment - 1) or file_offset % 16384 != virtual % 16384:
                    raise ValueError('The Android library does not support 16 KiB pages')
        if not loads: raise ValueError('The Android library has no loadable segments')


def verify_file(path, expected):
    if not re.fullmatch(r'[0-9a-f]{64}', expected):
        raise ValueError('A complete SHA-256 digest is required')
    with Path(path).open('rb') as source:
        actual = hashlib.file_digest(source, 'sha256').hexdigest()
    if actual != expected:
        raise ValueError('The Android runtime download failed SHA-256 verification')


def extract_source(archive, destination, exclude_prefixes=()):
    destination = Path(destination)
    if destination.exists():
        raise ValueError('Runtime source destination already exists')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent, prefix='.android-source-') as temp:
        stage = Path(temp) / 'source'
        stage.mkdir()
        with tarfile.open(archive) as bundle:
            members = [member for member in bundle.getmembers() if not any(
                member.name == prefix or member.name.startswith(prefix + '/') for prefix in exclude_prefixes)]
            for member in members:
                path = PurePosixPath(member.name)
                if path.is_absolute() or '..' in path.parts or '\\' in member.name:
                    raise ValueError('Unsafe runtime archive path')
                if member.isdev() or member.isfifo():
                    raise ValueError('Runtime archives cannot contain host devices')
                if member.issym() or member.islnk():
                    link = PurePosixPath(member.linkname)
                    base = path.parent if member.issym() else PurePosixPath()
                    if link.is_absolute() or not (stage / base / link).resolve().is_relative_to(stage.resolve()):
                        raise ValueError('Runtime archive link escapes its source directory')
            try:
                bundle.extractall(stage, members=members, filter='data')
            except tarfile.FilterError as error:
                raise ValueError('Unsafe runtime source archive') from error
        stage.rename(destination)
