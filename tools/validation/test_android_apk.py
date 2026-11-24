"""Adversarial synthetic ELF/ZIP fixtures. Explicit unittest invocation only."""
import struct
import tempfile
import unittest
import warnings
from pathlib import Path
import zipfile

from android_apk import InvalidArtifact, inspect_apk, inspect_elf


def elf(machine=183, align=16384, file_size=256, memory_size=256, phoff=64, phnum=1, offset=0, address=0):
    data = bytearray(256)
    data[:16] = b"\x7fELF\x02\x01\x01" + bytes(9)
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 3, machine, 1, 0, phoff, 0, 0, 64, 56, phnum, 0, 0, 0)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, offset, address, 0, file_size, memory_size, align)
    return bytes(data)


class ElfTests(unittest.TestCase):
    def test_valid_arm64_and_x86_64_headers(self):
        inspect_elf(elf(), "arm64-v8a")
        inspect_elf(elf(machine=62), "x86_64")

    def test_rejects_architecture_class_endianness_and_truncation(self):
        corrupt = []
        for index, value in [(0, 0), (4, 1), (5, 2), (6, 0)]:
            data = bytearray(elf())
            data[index] = value
            corrupt.append(bytes(data))
        corrupt += [elf(machine=62), elf()[:10], elf()[:63], elf()[:100]]
        for data in corrupt:
            with self.subTest(data=data[:16]), self.assertRaises(InvalidArtifact):
                inspect_elf(data, "arm64-v8a")

    def test_rejects_load_and_program_table_attacks(self):
        for options in [dict(align=4096), dict(align=24576), dict(phoff=2**63), dict(phnum=0),
                        dict(phnum=4097), dict(file_size=257), dict(memory_size=1),
                        dict(offset=1, file_size=1, address=0), dict(address=2**64-1)]:
            with self.subTest(options=options), self.assertRaises(InvalidArtifact):
                inspect_elf(elf(**options), "arm64-v8a")


class ApkTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="paranoic-apk-test-")
        self.addCleanup(self.temp.cleanup)
        self.apk = Path(self.temp.name) / "synthetic.apk"

    def write(self, entries, compression=zipfile.ZIP_DEFLATED):
        with zipfile.ZipFile(self.apk, "w", compression=compression) as archive:
            for name, data in entries:
                archive.writestr(name, data)

    def core(self):
        return [(f"lib/arm64-v8a/{name}", elf()) for name in ("libparanoic_bridge.so", "libjnidispatch.so")]

    def test_extracted_profile_reads_but_never_extracts(self):
        self.write(self.core())
        before = self.apk.read_bytes()
        self.assertEqual(2, len(inspect_apk(self.apk, ["arm64-v8a"])))
        self.assertEqual(before, self.apk.read_bytes())
        self.assertEqual([self.apk], list(self.apk.parent.iterdir()))

    def test_missing_pdf_runtime_or_partial_abi_fails(self):
        self.write(self.core())
        for abis, profile in [(["arm64-v8a"], "pdf"), (["arm64-v8a", "x86_64"], "core")]:
            with self.subTest(abis=abis, profile=profile), self.assertRaises(InvalidArtifact):
                inspect_apk(self.apk, abis, profile)

    def test_unsafe_duplicate_and_unexpected_abi_entries_fail(self):
        for name in ["../escape", "/absolute", "a\\b", "a/./b", "C:/drive", "lib/x86_64/orphan.so",
                     "lib/arm64-v8a/libparanoic_bridge.so"]:
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)  # intentional duplicate ZIP fixture
                self.write(self.core() + [(name, elf())])
            with self.subTest(name=name), self.assertRaises(InvalidArtifact):
                inspect_apk(self.apk, ["arm64-v8a"])

    def test_direct_profile_rejects_compressed_or_unaligned_libraries(self):
        for compression in (zipfile.ZIP_DEFLATED, zipfile.ZIP_STORED):
            self.write(self.core(), compression)
            with self.subTest(compression=compression), self.assertRaises(InvalidArtifact):
                inspect_apk(self.apk, ["arm64-v8a"], packaging="direct")

    def test_direct_profile_accepts_deliberately_aligned_stored_fixtures(self):
        with zipfile.ZipFile(self.apk, "w") as archive:
            for name, data in self.core():
                entry = zipfile.ZipInfo(name)
                offset = archive.fp.tell() + 30 + len(name.encode("ascii"))
                padding = (-offset) % 16384
                if padding < 4:
                    padding += 16384
                entry.extra = struct.pack("<HH", 0xCAFE, padding - 4) + bytes(padding - 4)
                archive.writestr(entry, data)
        self.assertEqual(2, len(inspect_apk(self.apk, ["arm64-v8a"], packaging="direct")))


if __name__ == "__main__":
    unittest.main()
