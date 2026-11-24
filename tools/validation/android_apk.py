#!/usr/bin/env python3
"""Manual inspection of an EXISTING APK: native inventory, ELF bounds and 16 KiB.
No builds, adb, execution, extraction or network. Python 3.10+ standard library.
Not signature verification, FFI execution or proof of PDF process isolation.
Importing this module performs no inspection.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import stat
import struct
import zipfile

ABIS = {"arm64-v8a": (2, 183), "x86_64": (2, 62), "armeabi-v7a": (1, 40)}
MAX_NATIVE = 64 * 1024 * 1024
PAGE = 16384


class InvalidArtifact(ValueError):
    pass


def need(condition: bool, message: str) -> None:
    if not condition:
        raise InvalidArtifact(message)


def inspect_elf(data: bytes, abi: str) -> None:
    need(abi in ABIS, "unsupported ABI")
    need(16 <= len(data) <= MAX_NATIVE, "ELF size outside inspection limit")
    elf_class, machine = ABIS[abi]
    need(data[:4] == b"\x7fELF", "missing ELF magic")
    need(data[4] == elf_class and data[5] == 1 and data[6] == 1, "ELF class/endianness/version mismatch")
    header = "<HHIQQQIHHHHHH" if elf_class == 2 else "<HHIIIIIHHHHHH"
    header_size = 16 + struct.calcsize(header)
    need(len(data) >= header_size, "truncated ELF header")
    fields = struct.unpack_from(header, data, 16)
    kind, actual_machine, version = fields[:3]
    phoff, ehsize, phentsize, phnum = fields[4], fields[7], fields[8], fields[9]
    need(kind == 3 and actual_machine == machine and version == 1, "expected ET_DYN/PIE for selected ABI")
    program = "<IIQQQQQQ" if elf_class == 2 else "<IIIIIIII"
    need(ehsize == header_size and phentsize == struct.calcsize(program), "unexpected ELF header layout")
    need(1 <= phnum <= 4096 and phoff >= header_size, "invalid ELF program table")
    need(phoff + phnum * phentsize <= len(data), "program table outside ELF")
    loads = 0
    for index in range(phnum):
        entry = struct.unpack_from(program, data, phoff + index * phentsize)
        if entry[0] != 1:
            continue
        loads += 1
        if elf_class == 2:
            offset, address, file_size, memory_size, align = entry[2], entry[3], entry[5], entry[6], entry[7]
        else:
            offset, address, file_size, memory_size, align = entry[1], entry[2], entry[4], entry[5], entry[7]
        need(file_size <= memory_size and offset + file_size <= len(data), "LOAD bounds outside ELF")
        need(address + memory_size <= 1 << (64 if elf_class == 2 else 32), "LOAD address overflow")
        need(align >= PAGE and align & (align - 1) == 0, "LOAD alignment below 16 KiB or not a power of two")
        need(offset % align == address % align, "LOAD offset/address incongruent")
    need(loads > 0, "ELF has no LOAD segments")


def _direct_offset(apk: Path, entry: zipfile.ZipInfo) -> int:
    with apk.open("rb") as stream:
        stream.seek(entry.header_offset)
        header = stream.read(30)
    need(len(header) == 30 and header[:4] == b"PK\x03\x04", "invalid ZIP local header")
    name_size, extra_size = struct.unpack_from("<HH", header, 26)
    return entry.header_offset + 30 + name_size + extra_size


def inspect_apk(apk: Path, abis: list[str], profile: str = "core", packaging: str = "extracted") -> list[str]:
    need(bool(abis) and len(set(abis)) == len(abis) and set(abis) <= ABIS.keys(), "invalid requested ABIs")
    need(profile in ("core", "pdf") and packaging in ("extracted", "direct"), "invalid inspection profile")
    required = {"libparanoic_bridge.so", "libjnidispatch.so"}
    if profile == "pdf":
        required |= {"libparanoic_pdf_worker.so", "libpdfium.so"}
    with zipfile.ZipFile(apk) as archive:
        entries = archive.infolist()
        need(len(entries) <= 10000, "too many ZIP entries")
        names: set[str] = set()
        native: dict[str, zipfile.ZipInfo] = {}
        total = 0
        for entry in entries:
            name = entry.filename
            pieces = name.rstrip("/").split("/")
            need(bool(name) and "\\" not in name and all(p not in ("", ".", "..") for p in pieces)
                 and ":" not in pieces[0], "unsafe ZIP entry name")
            need(name not in names, "duplicate ZIP entry")
            names.add(name)
            need(not entry.flag_bits & 1, "encrypted APK entry")
            need(not stat.S_ISLNK(entry.external_attr >> 16), "symlink APK entry")
            total += entry.file_size
            need(total <= 1024 * 1024 * 1024, "APK expanded-size inspection limit")
            if name.startswith("lib/") and name.endswith(".so"):
                need(len(pieces) == 3, "unexpected native library path")
                need(pieces[1] in abis, "unexpected ABI (possibly an incomplete native library set)")
                need(entry.file_size <= MAX_NATIVE, "native library size inspection limit")
                native[name] = entry
        expected = {f"lib/{abi}/{library}" for abi in abis for library in required}
        need(expected <= native.keys(), "missing required native libraries: " + ", ".join(sorted(expected - native.keys())))
        inspected = []
        for name, entry in sorted(native.items()):
            with archive.open(entry) as stream:
                data = stream.read(MAX_NATIVE + 1)
            need(len(data) == entry.file_size and len(data) <= MAX_NATIVE, "native library length mismatch")
            inspect_elf(data, name.split("/")[1])
            if packaging == "direct":
                need(entry.compress_type == zipfile.ZIP_STORED, "direct-load native library is compressed")
                need(_direct_offset(apk, entry) % PAGE == 0, "direct-load ZIP offset is not 16 KiB aligned")
            inspected.append(name)
        return inspected


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("--abis", required=True, help="exact packaged native ABI set, comma-separated")
    parser.add_argument("--profile", choices=("core", "pdf"), default="core")
    parser.add_argument("--packaging", choices=("extracted", "direct"), default="extracted",
                        help="extracted matches useLegacyPackaging=true; direct also checks ZIP offsets")
    args = parser.parse_args()
    try:
        inspected = inspect_apk(args.apk, args.abis.split(","), args.profile, args.packaging)
    except (InvalidArtifact, OSError, zipfile.BadZipFile, RuntimeError, NotImplementedError) as error:
        parser.exit(1, f"APK inspection failed: {error}\n")
    print(f"Inspected {len(inspected)} native ELF files ({args.profile}, {args.packaging}).")
    for name in inspected:
        print(name)
    print("Packaging checks only; device execution, signatures and PDF sandbox remain unverified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
