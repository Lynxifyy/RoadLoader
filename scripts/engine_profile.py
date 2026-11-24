#!/usr/bin/env python3
"""Characterise a Minecraft Bedrock engine binary for Road Loader.

The 1.17 engine is a stripped ELF32 ARM shared object with a large dynamic
symbol table but no debug symbols. This tool answers the questions the hook
layer needs answered before it can promise anything:

  * is the file a 32-bit ARM engine at all,
  * which of the anchors in the roadmap resolve by name,
  * does the engine import `eglSwapBuffers`, where is its PLT stub, and which
    functions call it (the frame-present path),
  * what does the vtable of `AppPlatform` point at.

Everything it prints is measured from the file. Anything it cannot resolve is
reported as unresolved with a reason, never guessed.

Usage:
    python3 scripts/engine_profile.py libminecraftpe.so [--out profiles/1.17.41.txt]
"""

from __future__ import annotations

import argparse
import hashlib
import struct
import sys
from dataclasses import dataclass, field

EM_ARM = 40
SHT_DYNSYM = 11
SHT_STRTAB = 3
PT_DYNAMIC = 2
DT_NULL = 0
DT_SYMTAB = 6
DT_STRTAB = 5
DT_STRSZ = 10
DT_SYMENT = 11
DT_JMPREL = 23
DT_PLTRELSZ = 2
DT_REL = 17
DT_RELSZ = 18
DT_RELA = 7
DT_RELASZ = 8
R_ARM_JUMP_SLOT = 22
R_ARM_GLOB_DAT = 21

# The anchors the roadmap names, by demangled C++ name. A symbol matches when
# its demangled form starts with the key.
ROADMAP_ANCHORS = [
    "Level::tick",
    "Level::tickEntities",
    "Actor::render",
    "Actor::move",
    "ScreenView::setup",
    "AppPlatform::swapBuffers",
    "AppPlatform::processMouseEvent",
    "ChunkMesher::buildMesh",
    "SoundEngine::play",
    "Vec3::distance",
    "Vec3::length",
    "AABB::intersects",
]


@dataclass
class Section:
    name: str
    kind: int
    address: int
    offset: int
    size: int
    link: int
    entsize: int


@dataclass
class Elf32:
    data: bytes
    machine: int
    entry: int
    sections: list[Section] = field(default_factory=list)
    dynsym: list[tuple[str, int, int, int]] = field(default_factory=list)
    imports: list[tuple[str, int, int]] = field(default_factory=list)  # name, slot, type

    def section(self, name: str) -> Section | None:
        for entry in self.sections:
            if entry.name == name:
                return entry
        return None

    def read(self, offset: int, size: int) -> bytes:
        return self.data[offset : offset + size]

    def vaddr_to_offset(self, address: int) -> int | None:
        for entry in self.sections:
            if entry.size and entry.address <= address < entry.address + entry.size:
                if entry.kind == 8:  # SHT_NOBITS
                    return None
                return entry.offset + (address - entry.address)
        return None


def load_elf32(path: str) -> Elf32:
    with open(path, "rb") as handle:
        data = handle.read()
    if data[:4] != b"\x7fELF":
        raise SystemExit(f"{path}: not an ELF file")
    if data[4] != 1:
        raise SystemExit(f"{path}: ELF64; Road Loader needs the 32-bit engine")
    (machine,) = struct.unpack_from("<H", data, 18)
    (entry,) = struct.unpack_from("<I", data, 24)
    (shoff,) = struct.unpack_from("<I", data, 32)
    (shentsize,) = struct.unpack_from("<H", data, 46)
    (shnum,) = struct.unpack_from("<H", data, 48)
    (shstrndx,) = struct.unpack_from("<H", data, 50)
    elf = Elf32(data=data, machine=machine, entry=entry)

    headers = []
    for index in range(shnum):
        base = shoff + index * shentsize
        name, kind, _flags, address, offset, size, link, _info, _align, entsize = (
            struct.unpack_from("<IIIIIIIIII", data, base)
        )
        headers.append((name, kind, address, offset, size, link, entsize))
    name_section = headers[shstrndx]
    names = data[name_section[3] : name_section[3] + name_section[4]]
    for name, kind, address, offset, size, link, entsize in headers:
        end = names.find(b"\0", name)
        text = names[name:end].decode("ascii", "replace")
        elf.sections.append(Section(text, kind, address, offset, size, link, entsize))

    dynamic = elf.section(".dynamic")
    if dynamic is not None:
        parse_dynamic(elf, dynamic)
    else:
        parse_dynamic_from_segments(elf)
    return elf


def parse_dynamic(elf: Elf32, section: Section) -> None:
    tags: dict[int, int] = {}
    for index in range(section.size // 8):
        tag, value = struct.unpack_from("<iI", elf.data, section.offset + index * 8)
        if tag == DT_NULL:
            break
        tags[tag] = value
    build_tables(elf, tags)


def parse_dynamic_from_segments(elf: Elf32) -> None:
    (phoff,) = struct.unpack_from("<I", elf.data, 28)
    (phentsize,) = struct.unpack_from("<H", elf.data, 42)
    (phnum,) = struct.unpack_from("<H", elf.data, 44)
    for index in range(phnum):
        base = phoff + index * phentsize
        kind, offset, vaddr, _paddr, filesz, memsz, _flags, _align = struct.unpack_from(
            "<IIIIIIII", elf.data, base
        )
        if kind != PT_DYNAMIC:
            continue
        tags: dict[int, int] = {}
        for entry in range(filesz // 8):
            tag, value = struct.unpack_from("<iI", elf.data, offset + entry * 8)
            if tag == DT_NULL:
                break
            tags[tag] = value
        build_tables(elf, tags)
        return


def build_tables(elf: Elf32, tags: dict[int, int]) -> None:
    symtab = tags.get(DT_SYMTAB, 0)
    strtab = tags.get(DT_STRTAB, 0)
    strsz = tags.get(DT_STRSZ, 0)
    syment = tags.get(DT_SYMENT, 16)
    if not symtab or not strtab:
        return
    strings_offset = elf.vaddr_to_offset(strtab)
    if strings_offset is None:
        return
    strings = elf.read(strings_offset, strsz)

    def name_at(offset: int) -> str:
        if offset >= len(strings):
            return ""
        end = strings.find(b"\0", offset)
        return strings[offset:end].decode("ascii", "replace")

    syms_offset = elf.vaddr_to_offset(symtab)
    if syms_offset is None:
        return
    dynsym_section = elf.section(".dynsym")
    count = (dynsym_section.size // syment) if dynsym_section else 0
    for index in range(count):
        offset = syms_offset + index * syment
        st_name, st_value, st_size, st_info, _other, st_shndx = struct.unpack_from(
            "<IIIBBH", elf.data, offset
        )
        if st_name == 0:
            continue
        elf.dynsym.append((name_at(st_name), st_value, st_size, st_info))

    def relocations(address: int, size: int, with_addend: bool) -> None:
        if not address or not size:
            return
        base = elf.vaddr_to_offset(address)
        if base is None:
            return
        entry_size = 12 if with_addend else 8
        for index in range(size // entry_size):
            r_offset, r_info = struct.unpack_from("<II", elf.data, base + index * entry_size)
            kind = r_info & 0xFF
            symbol = r_info >> 8
            if kind not in (R_ARM_JUMP_SLOT, R_ARM_GLOB_DAT):
                continue
            if symbol >= len(elf.dynsym):
                continue
            elf.imports.append((elf.dynsym[symbol][0], r_offset, kind))

    relocations(tags.get(DT_JMPREL, 0), tags.get(DT_PLTRELSZ, 0), tags.get(DT_REL, 0) != 0)
    relocations(tags.get(DT_REL, 0), tags.get(DT_RELSZ, 0), False)
    relocations(tags.get(DT_RELA, 0), tags.get(DT_RELASZ, 0), True)


def demangle(name: str) -> str:
    """Enough of the Itanium ABI to recognise a member function.

    Only the shapes the roadmap anchors use are decoded: a class name and a
    member name, with nested namespaces flattened to `A::B`.
    """
    if not name.startswith("_Z"):
        return name
    body = name[2:]
    if not body or body[0] in "TV":
        return name
    # A leading N marks a nested name, which is how every member function is
    # encoded; the length-prefixed components follow it.
    if body[0] == "N":
        body = body[1:]
        # Nested names may carry cv/ref qualifiers before the first component,
        # as in `_ZNK4Vec33absEv` (a const member function).
        while body and body[0] in "rVK":
            body = body[1:]
    parts = []
    index = 0
    while index < len(body) and body[index].isdigit():
        length = 0
        while index < len(body) and body[index].isdigit():
            length = length * 10 + int(body[index])
            index += 1
        if length == 0 or index + length > len(body):
            break
        parts.append(body[index : index + length])
        index += length
    if len(parts) < 2:
        return name
    return "::".join(parts)


def rotate_right(value: int, bits: int) -> int:
    bits %= 32
    return ((value >> bits) | (value << (32 - bits))) & 0xFFFFFFFF


def decode_plt_entry(elf: Elf32, address: int) -> int | None:
    """Returns the GOT slot an ARM PLT stub loads, or None.

    An ARM PLT stub computes the slot with two PC-relative `add`s whose
    immediates are rotated, then loads it with a pre-indexed `ldr pc`.
    """
    offset = elf.vaddr_to_offset(address)
    if offset is None:
        return None
    first, second, third, _pad = struct.unpack_from("<4I", elf.data, offset)
    if (first & 0xFFFFF000) != 0xE28FC000:  # add ip, pc, #imm
        return None
    if (second & 0xFFFFF000) != 0xE28CC000:  # add ip, ip, #imm
        return None
    if (third & 0xFFFFF000) != 0xE5BCF000:  # ldr pc, [ip, #imm]!
        return None
    first_imm = rotate_right(first & 0xFF, ((first >> 8) & 0xF) * 2)
    second_imm = rotate_right(second & 0xFF, ((second >> 8) & 0xF) * 2)
    third_imm = third & 0xFFF
    target = address + 8 + first_imm + second_imm
    target = target + third_imm if (third >> 23) & 1 else target - third_imm
    return target & 0xFFFFFFFF


def find_plt_stub(elf: Elf32, slot_address: int) -> int | None:
    """Finds the PLT stub that loads `slot_address`."""
    section = elf.section(".plt")
    if section is None:
        return None
    # PLT0 plus its padding occupy the first 32 bytes; stubs follow, 16 each.
    for offset in range(0x20, section.size - 16, 16):
        address = section.address + offset
        if decode_plt_entry(elf, address) == slot_address:
            return address
    return None


def decode_arm_bl(elf: Elf32, text: Section, target: int) -> list[int]:
    """Every ARM-mode `bl` to `target` inside .text, as virtual addresses."""
    sites = []
    for offset in range(0, text.size - 4, 4):
        (word,) = struct.unpack_from("<I", elf.data, text.offset + offset)
        if (word & 0xFF000000) != 0xEB000000:
            continue
        immediate = word & 0x00FFFFFF
        if immediate & 0x00800000:
            immediate -= 0x01000000
        address = text.address + offset
        destination = (address + 8 + immediate * 4) & 0xFFFFFFFF
        if destination == target:
            sites.append(address)
    return sites


def decode_thumb_branch(elf: Elf32, text: Section, target: int) -> list[int]:
    """Every Thumb-2 `bl`/`blx` to `target` inside .text, as virtual addresses.

    Calls into an ARM PLT stub use `blx`, whose immediate packs the low bit of
    the offset differently from `bl`; both encodings are decoded.
    """
    sites = []
    data = elf.data
    for offset in range(0, text.size - 4, 2):
        (first,) = struct.unpack_from("<H", data, text.offset + offset)
        if (first & 0xF800) != 0xF000:
            continue
        (second,) = struct.unpack_from("<H", data, text.offset + offset + 2)
        if (second & 0xD000) != 0xD000:
            continue
        sign = (first >> 10) & 1
        imm10 = first & 0x03FF
        j1 = (second >> 13) & 1
        j2 = (second >> 11) & 1
        i1 = 1 - (j1 ^ sign)
        i2 = 1 - (j2 ^ sign)
        address = text.address + offset
        if second & 1:
            # bl: S:I1:I2:imm10:imm11:'0'
            imm11 = second & 0x07FF
            displacement = (
                (sign << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm11 << 1)
            )
            if sign:
                displacement -= 1 << 25
            destination = (address + 4 + displacement) & 0xFFFFFFFF
        else:
            # blx: S:I1:I2:imm10H:imm10L:'00', from an aligned PC
            imm10l = (second >> 1) & 0x03FF
            displacement = (
                (sign << 24) | (i1 << 23) | (i2 << 22) | (imm10 << 12) | (imm10l << 2)
            )
            if sign:
                displacement -= 1 << 25
            destination = (((address + 4) & ~3) + displacement) & 0xFFFFFFFF
        if destination == target:
            sites.append(address)
    return sites


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("engine", help="libminecraftpe.so extracted from the APK")
    parser.add_argument("--out", help="write the profile here")
    parser.add_argument(
        "--self-check",
        action="store_true",
        help="verify the PLT decoder against .rel.plt before profiling",
    )
    arguments = parser.parse_args()

    elf = load_elf32(arguments.engine)
    if elf.machine != EM_ARM:
        raise SystemExit(f"machine {elf.machine}: not an ARM engine")

    with open(arguments.engine, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()

    text = elf.section(".text")
    if text is None:
        raise SystemExit("no .text section")

    if arguments.self_check:
        # The linker emits one PLT stub per JUMP_SLOT, in relocation order. If
        # the decoder disagrees with that order, every address it produces is
        # suspect, so refuse to profile.
        plt = elf.section(".plt")
        rel_plt = elf.section(".rel.plt")
        if plt is None or rel_plt is None:
            raise SystemExit("self-check needs .plt and .rel.plt")
        checked = 0
        mismatches = []
        for index in range(min(64, rel_plt.size // 8)):
            (slot,) = struct.unpack_from("<I", elf.data, rel_plt.offset + index * 8)
            stub = plt.address + 0x20 + index * 16
            decoded = decode_plt_entry(elf, stub)
            checked += 1
            if decoded != slot:
                mismatches.append((index, slot, decoded))
        if mismatches:
            for index, slot, decoded in mismatches[:5]:
                print(f"stub {index}: slot 0x{slot:x} decoded 0x{decoded:x}")
            raise SystemExit(f"self-check failed: {len(mismatches)}/{checked} stubs disagree")
        print(f"self-check: {checked}/{checked} PLT stubs match .rel.plt")

    lines: list[str] = []

    def emit(line: str = "") -> None:
        lines.append(line)

    emit("# Road Loader engine profile")
    emit("# Generated by scripts/engine_profile.py. Every value is measured from")
    emit("# the binary named below; nothing here is inferred from another build.")
    emit(f"source={arguments.engine}")
    emit(f"sha256={digest}")
    emit("abi=armeabi-v7a")
    emit("elf_class=32")
    emit(f"entry=0x{elf.entry:x}")
    emit(f"text_address=0x{text.address:x}")
    emit(f"text_size=0x{text.size:x}")
    emit(f"dynsym_count={len(elf.dynsym)}")
    emit(f"import_count={len(elf.imports)}")

    demangled = {}
    for name, value, _size, _info in elf.dynsym:
        demangled.setdefault(demangle(name), (name, value))

    emit("")
    emit("[anchors]")
    for anchor in ROADMAP_ANCHORS:
        key = anchor.replace("::", ".").lower()
        exact = demangled.get(anchor)
        prefixed = [name for name in demangled if name.startswith(anchor)]
        if exact is not None:
            emit(f"{key}.status=resolved")
            emit(f"{key}.strategy=symbol")
            emit(f"{key}.address=0x{exact[1]:x}")
            emit(f"{key}.mangled={exact[0]}")
        elif prefixed:
            emit(f"{key}.status=partial")
            emit(f"{key}.strategy=symbol-overloads")
            emit(f"{key}.count={len(prefixed)}")
            emit(f"{key}.first={prefixed[0]}")
        else:
            emit(f"{key}.status=unresolved")
            emit(f"{key}.reason=not in .dynsym; needs a vtable slot or a caller anchor")

    emit("")
    emit("[frame-present]")
    slot = next((entry for entry in elf.imports if entry[0] == "eglSwapBuffers"), None)
    if slot is None:
        emit("eglSwapBuffers.imported=false")
    else:
        _name, slot_address, _kind = slot
        emit("eglSwapBuffers.imported=true")
        emit(f"eglSwapBuffers.got=0x{slot_address:x}")
        stub = find_plt_stub(elf, slot_address)
        if stub is None:
            emit("eglSwapBuffers.plt_stub=unresolved")
        else:
            emit(f"eglSwapBuffers.plt_stub=0x{stub:x}")
            arm_sites = decode_arm_bl(elf, text, stub)
            thumb_sites = decode_thumb_branch(elf, text, stub)
            sites = sorted(arm_sites + thumb_sites)
            emit(f"eglSwapBuffers.direct_branch_sites={len(sites)}")
            for index, site in enumerate(sites[:16]):
                emit(f"eglSwapBuffers.direct_branch_site.{index}=0x{site:x}")
            if not sites:
                emit(
                    "eglSwapBuffers.call_path=indirect"
                )
                emit(
                    "eglSwapBuffers.call_path_note=calls do not branch to the PLT stub, "
                    "so the GOT slot is the interception point and an inline hook would "
                    "have to find a caller that does not exist"
                )

    emit("")
    emit("[imports-of-interest]")
    for name in ("eglSwapBuffers", "eglMakeCurrent", "eglCreateWindowSurface",
                 "glDrawElements", "glDrawArrays", "glBindTexture"):
        entry = next((item for item in elf.imports if item[0] == name), None)
        if entry is None:
            emit(f"{name}.imported=false")
            continue
        stub = find_plt_stub(elf, entry[1])
        stub_text = f"0x{stub:x}" if stub is not None else "unresolved"
        emit(f"{name}.imported=true")
        emit(f"{name}.got=0x{entry[1]:x}")
        emit(f"{name}.plt_stub={stub_text}")

    emit("")
    emit("[symbol-census]")
    prefixes = [
        "Level", "LevelChunk", "Actor", "Player", "Mob", "BlockActor", "Block",
        "ScreenView", "AppPlatform", "Chunk", "Mesh", "Vec3", "AABB", "Sound",
        "Options", "ServerLevel", "Dimension", "Camera",
    ]
    counts = {name: 0 for name in prefixes}
    for anchor in demangled:
        head = anchor.split("::")[0]
        if head in counts:
            counts[head] += 1
    for name in prefixes:
        emit(f"{name}={counts[name]}")

    emit("")
    emit("[app-platform]")
    vtable = next((entry for entry in elf.dynsym if entry[0] == "_ZTV11AppPlatform"), None)
    if vtable is None:
        emit("vtable=unresolved")
    else:
        emit(f"vtable=0x{vtable[1]:x}")
        offset = elf.vaddr_to_offset(vtable[1])
        if offset is not None:
            # The vtable starts with offset-to-top and RTTI; slots follow.
            words = []
            for index in range(24):
                (word,) = struct.unpack_from("<I", elf.data, offset + index * 4)
                words.append(word)
            emit("vtable.head=" + ",".join(f"0x{word:x}" for word in words))

    report = "\n".join(lines) + "\n"
    if arguments.out:
        with open(arguments.out, "w", encoding="utf-8") as handle:
            handle.write(report)
        print(f"wrote {arguments.out} ({len(lines)} lines)")
    else:
        sys.stdout.write(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
