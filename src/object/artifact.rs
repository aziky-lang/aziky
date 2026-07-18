#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugSource {
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugDeclaration {
    pub name: String,
    pub kind: String,
    pub source_index: usize,
    pub line: usize,
    pub column: usize,
    pub public: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactMetadata {
    pub target: String,
    pub sources: Vec<DebugSource>,
    pub declarations: Vec<DebugDeclaration>,
    pub block_symbols: Vec<(usize, usize, usize)>,
}

#[derive(Clone)]
struct Section {
    name: &'static str,
    ty: u32,
    flags: u64,
    align: u64,
    entsize: u64,
    link: u32,
    info: u32,
    data: Vec<u8>,
    offset: u64,
    address: u64,
}

const ELF_HEADER_SIZE: usize = 64;
const ELF_SECTION_HEADER_SIZE: usize = 64;
const ELF_PROGRAM_HEADER_SIZE: usize = 56;
const EM_X86_64: u16 = 62;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_HASH: u32 = 5;
const SHT_DYNAMIC: u32 = 6;
const SHT_NOTE: u32 = 7;
const SHT_DYNSYM: u32 = 11;
const SHF_ALLOC: u64 = 2;
const SHF_EXECINSTR: u64 = 4;

pub fn emit_elf64_relocatable(code: &[u8], metadata: &ArtifactMetadata) -> Vec<u8> {
    let debug = DebugSections::new(code.len(), metadata);
    let (strtab, symtab, first_global) = elf_symbols(code.len(), metadata, false);
    let mut sections = vec![
        null_section(),
        section(
            ".text",
            SHT_PROGBITS,
            SHF_ALLOC | SHF_EXECINSTR,
            16,
            code.to_vec(),
        ),
        section(".note.aziky", SHT_NOTE, 0, 4, aziky_note(metadata)),
        section(".debug_abbrev", SHT_PROGBITS, 0, 1, debug.abbrev),
        section(".debug_info", SHT_PROGBITS, 0, 1, debug.info),
        section(".debug_line", SHT_PROGBITS, 0, 1, debug.line),
        section(".debug_str", SHT_PROGBITS, 0, 1, debug.strings),
        Section {
            name: ".symtab",
            ty: SHT_SYMTAB,
            flags: 0,
            align: 8,
            entsize: 24,
            link: 8,
            info: first_global,
            data: symtab,
            offset: 0,
            address: 0,
        },
        section(".strtab", SHT_STRTAB, 0, 1, strtab),
        section(".shstrtab", SHT_STRTAB, 0, 1, Vec::new()),
    ];
    finish_elf(1, &mut sections, 9, &[], 0)
}

pub fn emit_static_archive(object: &[u8], exported_symbols: &[&str]) -> Vec<u8> {
    let mut names = exported_symbols.to_vec();
    names.sort_unstable();
    names.dedup();
    let symbol_payload_len =
        4 + names.len() * 4 + names.iter().map(|name| name.len() + 1).sum::<usize>();
    let symbol_padding = symbol_payload_len % 2;
    let object_header_offset = 8 + 60 + symbol_payload_len + symbol_padding;

    let mut symbol_payload = Vec::with_capacity(symbol_payload_len);
    symbol_payload.extend_from_slice(&(names.len() as u32).to_be_bytes());
    for _ in &names {
        symbol_payload.extend_from_slice(&(object_header_offset as u32).to_be_bytes());
    }
    for name in &names {
        symbol_payload.extend_from_slice(name.as_bytes());
        symbol_payload.push(0);
    }

    let mut out = b"!<arch>\n".to_vec();
    append_ar_header(&mut out, "/", symbol_payload.len());
    out.extend_from_slice(&symbol_payload);
    if out.len() % 2 != 0 {
        out.push(b'\n');
    }
    append_ar_header(&mut out, "aziky.o/", object.len());
    out.extend_from_slice(object);
    if out.len() % 2 != 0 {
        out.push(b'\n');
    }
    out
}

fn append_ar_header(out: &mut Vec<u8>, name: &str, size: usize) {
    append_padded_ascii(out, name, 16);
    append_padded_ascii(out, "0", 12);
    append_padded_ascii(out, "0", 6);
    append_padded_ascii(out, "0", 6);
    append_padded_ascii(out, "100644", 8);
    append_padded_ascii(out, &size.to_string(), 10);
    out.extend_from_slice(b"`\n");
}

fn append_padded_ascii(out: &mut Vec<u8>, value: &str, width: usize) {
    assert!(value.len() <= width);
    out.extend_from_slice(value.as_bytes());
    out.resize(out.len() + width - value.len(), b' ');
}

pub fn emit_elf64_shared(code: &[u8], metadata: &ArtifactMetadata) -> Vec<u8> {
    let debug = DebugSections::new(code.len(), metadata);
    let mut dynstr = vec![0];
    let dyn_name = push_string(&mut dynstr, "aziky_program_entry");
    let soname = push_string(&mut dynstr, "libaziky.so");
    let mut dynsym = vec![0; 24];
    append_elf_symbol(&mut dynsym, dyn_name, 0x12, 0, 1, 0, code.len() as u64);
    let hash = sysv_hash_table("aziky_program_entry");
    let (strtab, symtab, first_global) = elf_symbols(code.len(), metadata, false);
    let mut sections = vec![
        null_section(),
        section(
            ".text",
            SHT_PROGBITS,
            SHF_ALLOC | SHF_EXECINSTR,
            16,
            code.to_vec(),
        ),
        section(".note.aziky", SHT_NOTE, SHF_ALLOC, 4, aziky_note(metadata)),
        Section {
            name: ".hash",
            ty: SHT_HASH,
            flags: SHF_ALLOC,
            align: 8,
            entsize: 4,
            link: 4,
            info: 0,
            data: hash,
            offset: 0,
            address: 0,
        },
        Section {
            name: ".dynsym",
            ty: SHT_DYNSYM,
            flags: SHF_ALLOC,
            align: 8,
            entsize: 24,
            link: 5,
            info: 1,
            data: dynsym,
            offset: 0,
            address: 0,
        },
        section(".dynstr", SHT_STRTAB, SHF_ALLOC, 1, dynstr),
        Section {
            name: ".dynamic",
            ty: SHT_DYNAMIC,
            flags: SHF_ALLOC,
            align: 8,
            entsize: 16,
            link: 5,
            info: 0,
            data: vec![0; 7 * 16],
            offset: 0,
            address: 0,
        },
        section(".debug_abbrev", SHT_PROGBITS, 0, 1, debug.abbrev),
        section(".debug_info", SHT_PROGBITS, 0, 1, debug.info),
        section(".debug_line", SHT_PROGBITS, 0, 1, debug.line),
        section(".debug_str", SHT_PROGBITS, 0, 1, debug.strings),
        Section {
            name: ".symtab",
            ty: SHT_SYMTAB,
            flags: 0,
            align: 8,
            entsize: 24,
            link: 12,
            info: first_global,
            data: symtab,
            offset: 0,
            address: 0,
        },
        section(".strtab", SHT_STRTAB, 0, 1, strtab),
        section(".shstrtab", SHT_STRTAB, 0, 1, Vec::new()),
    ];

    // Three program headers: one load image, one writable dynamic view, and GNU stack.
    let phnum = 3u16;
    layout_elf_sections(
        &mut sections,
        ELF_HEADER_SIZE + ELF_PROGRAM_HEADER_SIZE * phnum as usize,
        13,
        true,
    );
    let text_address = sections[1].address;
    sections[4].data[32..40].copy_from_slice(&text_address.to_le_bytes());
    for index in 2..sections[11].data.len() / 24 {
        let value_offset = index * 24 + 8;
        let current = u64::from_le_bytes(
            sections[11].data[value_offset..value_offset + 8]
                .try_into()
                .expect("ELF symbol value"),
        );
        sections[11].data[value_offset..value_offset + 8]
            .copy_from_slice(&current.saturating_add(text_address).to_le_bytes());
    }
    let dynamic_entries = [
        (4i64, sections[3].address),         // DT_HASH
        (5, sections[5].address),            // DT_STRTAB
        (6, sections[4].address),            // DT_SYMTAB
        (10, sections[5].data.len() as u64), // DT_STRSZ
        (11, 24),                            // DT_SYMENT
        (14, soname as u64),                 // DT_SONAME
        (0, 0),                              // DT_NULL
    ];
    sections[6].data.clear();
    for (tag, value) in dynamic_entries {
        sections[6].data.extend_from_slice(&tag.to_le_bytes());
        sections[6].data.extend_from_slice(&value.to_le_bytes());
    }

    let file_end = sections
        .iter()
        .map(|section| section.offset + section.data.len() as u64)
        .max()
        .unwrap_or(0);
    let program_headers = vec![
        program_header(1, 5, 0, 0, file_end, file_end, 0x1000),
        program_header(
            2,
            4,
            sections[6].offset,
            sections[6].address,
            sections[6].data.len() as u64,
            sections[6].data.len() as u64,
            8,
        ),
        program_header(0x6474_e551, 6, 0, 0, 0, 0, 16),
    ];
    finish_elf_prelaid(3, &sections, 13, &program_headers, 0)
}

fn program_header(
    ty: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
) -> [u8; 56] {
    let mut out = [0u8; 56];
    out[0..4].copy_from_slice(&ty.to_le_bytes());
    out[4..8].copy_from_slice(&flags.to_le_bytes());
    out[8..16].copy_from_slice(&offset.to_le_bytes());
    out[16..24].copy_from_slice(&vaddr.to_le_bytes());
    out[24..32].copy_from_slice(&vaddr.to_le_bytes());
    out[32..40].copy_from_slice(&filesz.to_le_bytes());
    out[40..48].copy_from_slice(&memsz.to_le_bytes());
    out[48..56].copy_from_slice(&align.to_le_bytes());
    out
}

fn sysv_hash_table(name: &str) -> Vec<u8> {
    let mut hash = 0u32;
    for byte in name.bytes() {
        hash = (hash << 4).wrapping_add(byte as u32);
        let high = hash & 0xf000_0000;
        if high != 0 {
            hash ^= high >> 24;
        }
        hash &= !high;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_le_bytes()); // nbucket
    out.extend_from_slice(&2u32.to_le_bytes()); // nchain
    out.extend_from_slice(&1u32.to_le_bytes()); // bucket[0]
    out.extend_from_slice(&0u32.to_le_bytes()); // chain[0]
    out.extend_from_slice(&0u32.to_le_bytes()); // chain[1]
    let _ = hash; // One bucket does not need the value, only the standard chain shape.
    out
}

fn elf_symbols(
    code_len: usize,
    metadata: &ArtifactMetadata,
    leading_underscore: bool,
) -> (Vec<u8>, Vec<u8>, u32) {
    let mut strings = vec![0];
    let mut symbols = vec![0; 24];
    // Local section symbol.
    append_elf_symbol(&mut symbols, 0, 0x03, 0, 1, 0, 0);
    for (block, start, end) in &metadata.block_symbols {
        let name = push_string(&mut strings, &format!("aziky.block.{block}"));
        append_elf_symbol(
            &mut symbols,
            name,
            0x02,
            0,
            1,
            *start as u64,
            end.saturating_sub(*start) as u64,
        );
    }
    let first_global = (symbols.len() / 24) as u32;
    for name in ["_start", "aziky_program_entry"] {
        let rendered = if leading_underscore {
            format!("_{name}")
        } else {
            name.to_string()
        };
        let offset = push_string(&mut strings, &rendered);
        append_elf_symbol(&mut symbols, offset, 0x12, 0, 1, 0, code_len as u64);
    }
    (strings, symbols, first_global)
}

fn append_elf_symbol(
    out: &mut Vec<u8>,
    name: u32,
    info: u8,
    other: u8,
    shndx: u16,
    value: u64,
    size: u64,
) {
    out.extend_from_slice(&name.to_le_bytes());
    out.push(info);
    out.push(other);
    out.extend_from_slice(&shndx.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
}

fn section(name: &'static str, ty: u32, flags: u64, align: u64, data: Vec<u8>) -> Section {
    Section {
        name,
        ty,
        flags,
        align,
        entsize: 0,
        link: 0,
        info: 0,
        data,
        offset: 0,
        address: 0,
    }
}

fn null_section() -> Section {
    section("", 0, 0, 0, Vec::new())
}

fn finish_elf(
    ty: u16,
    sections: &mut [Section],
    shstrndx: usize,
    program_headers: &[[u8; 56]],
    entry: u64,
) -> Vec<u8> {
    layout_elf_sections(
        sections,
        ELF_HEADER_SIZE + program_headers.len() * ELF_PROGRAM_HEADER_SIZE,
        shstrndx,
        false,
    );
    finish_elf_prelaid(ty, sections, shstrndx, program_headers, entry)
}

fn layout_elf_sections(
    sections: &mut [Section],
    start: usize,
    shstrndx: usize,
    allocate_addresses: bool,
) {
    let (shstr, offsets) = section_name_table(sections);
    sections[shstrndx].data = shstr;
    let mut offset = start as u64;
    for section in sections.iter_mut().skip(1) {
        offset = align_up(offset, section.align.max(1));
        section.offset = offset;
        section.address = if allocate_addresses && section.flags & SHF_ALLOC != 0 {
            offset
        } else {
            0
        };
        offset += section.data.len() as u64;
    }
    let _ = offsets;
}

fn finish_elf_prelaid(
    ty: u16,
    sections: &[Section],
    shstrndx: usize,
    program_headers: &[[u8; 56]],
    entry: u64,
) -> Vec<u8> {
    let (_, name_offsets) = section_name_table(sections);
    let data_end = sections
        .iter()
        .map(|section| section.offset + section.data.len() as u64)
        .max()
        .unwrap_or(ELF_HEADER_SIZE as u64);
    let shoff = align_up(data_end, 8);
    let mut out = vec![0u8; ELF_HEADER_SIZE + program_headers.len() * ELF_PROGRAM_HEADER_SIZE];
    write_elf_header(
        &mut out[..ELF_HEADER_SIZE],
        ty,
        entry,
        program_headers.len() as u16,
        shoff,
        sections.len() as u16,
        shstrndx as u16,
    );
    for (index, header) in program_headers.iter().enumerate() {
        let start = ELF_HEADER_SIZE + index * ELF_PROGRAM_HEADER_SIZE;
        out[start..start + ELF_PROGRAM_HEADER_SIZE].copy_from_slice(header);
    }
    for section in sections.iter().skip(1) {
        out.resize(section.offset as usize, 0);
        out.extend_from_slice(&section.data);
    }
    out.resize(shoff as usize, 0);
    for (index, section) in sections.iter().enumerate() {
        append_section_header(&mut out, name_offsets[index], section);
    }
    out
}

fn write_elf_header(
    out: &mut [u8],
    ty: u16,
    entry: u64,
    phnum: u16,
    shoff: u64,
    shnum: u16,
    shstrndx: u16,
) {
    out[0..4].copy_from_slice(b"\x7fELF");
    out[4] = 2;
    out[5] = 1;
    out[6] = 1;
    out[16..18].copy_from_slice(&ty.to_le_bytes());
    out[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
    out[20..24].copy_from_slice(&1u32.to_le_bytes());
    out[24..32].copy_from_slice(&entry.to_le_bytes());
    if phnum > 0 {
        out[32..40].copy_from_slice(&(ELF_HEADER_SIZE as u64).to_le_bytes());
    }
    out[40..48].copy_from_slice(&shoff.to_le_bytes());
    out[52..54].copy_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes());
    out[54..56].copy_from_slice(&(ELF_PROGRAM_HEADER_SIZE as u16).to_le_bytes());
    out[56..58].copy_from_slice(&phnum.to_le_bytes());
    out[58..60].copy_from_slice(&(ELF_SECTION_HEADER_SIZE as u16).to_le_bytes());
    out[60..62].copy_from_slice(&shnum.to_le_bytes());
    out[62..64].copy_from_slice(&shstrndx.to_le_bytes());
}

fn append_section_header(out: &mut Vec<u8>, name: u32, section: &Section) {
    out.extend_from_slice(&name.to_le_bytes());
    out.extend_from_slice(&section.ty.to_le_bytes());
    out.extend_from_slice(&section.flags.to_le_bytes());
    out.extend_from_slice(&section.address.to_le_bytes());
    out.extend_from_slice(&section.offset.to_le_bytes());
    out.extend_from_slice(&(section.data.len() as u64).to_le_bytes());
    out.extend_from_slice(&section.link.to_le_bytes());
    out.extend_from_slice(&section.info.to_le_bytes());
    out.extend_from_slice(&section.align.to_le_bytes());
    out.extend_from_slice(&section.entsize.to_le_bytes());
}

fn section_name_table(sections: &[Section]) -> (Vec<u8>, Vec<u32>) {
    let mut table = vec![0];
    let mut offsets = Vec::with_capacity(sections.len());
    for section in sections {
        offsets.push(push_string(&mut table, section.name));
    }
    (table, offsets)
}

fn push_string(table: &mut Vec<u8>, value: &str) -> u32 {
    let offset = table.len() as u32;
    table.extend_from_slice(value.as_bytes());
    table.push(0);
    offset
}

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 || value % align == 0 {
        value
    } else {
        value + align - value % align
    }
}

fn aziky_note(metadata: &ArtifactMetadata) -> Vec<u8> {
    let descriptor = metadata_text(metadata).into_bytes();
    let mut out = Vec::new();
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&(descriptor.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x415a_4b59u32.to_le_bytes());
    out.extend_from_slice(b"AZIKY\0");
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&descriptor);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

pub fn metadata_text(metadata: &ArtifactMetadata) -> String {
    let mut out = format!("aziky-metadata=1\ntarget={}\n", metadata.target);
    for (index, source) in metadata.sources.iter().enumerate() {
        out.push_str(&format!(
            "source\t{index}\t{}\n",
            escape_field(&source.path)
        ));
    }
    let mut declarations = metadata.declarations.clone();
    declarations.sort_by_key(|declaration| {
        (
            declaration.source_index,
            declaration.line,
            declaration.column,
            declaration.name.clone(),
        )
    });
    for declaration in declarations {
        out.push_str(&format!(
            "declaration\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            escape_field(&declaration.kind),
            usize::from(declaration.public),
            declaration.source_index,
            declaration.line,
            declaration.column,
            escape_field(&declaration.name),
            metadata
                .sources
                .get(declaration.source_index)
                .map_or("", |source| source.path.as_str())
        ));
    }
    out
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

struct DebugSections {
    abbrev: Vec<u8>,
    info: Vec<u8>,
    line: Vec<u8>,
    strings: Vec<u8>,
}

impl DebugSections {
    fn new(code_len: usize, metadata: &ArtifactMetadata) -> Self {
        let mut strings = Vec::new();
        let producer = push_string(&mut strings, "Aziky compiler");
        let unit_name = push_string(
            &mut strings,
            metadata
                .sources
                .first()
                .map_or("<aziky>", |source| source.path.as_str()),
        );
        let abbrev = vec![
            1, 0x11, 0, // abbrev 1, DW_TAG_compile_unit, no children
            0x25, 0x0e, // DW_AT_producer, DW_FORM_strp
            0x13, 0x05, // DW_AT_language, DW_FORM_data2
            0x03, 0x0e, // DW_AT_name, DW_FORM_strp
            0x10, 0x17, // DW_AT_stmt_list, DW_FORM_sec_offset
            0, 0, 0,
        ];
        let mut info_body = Vec::new();
        info_body.extend_from_slice(&4u16.to_le_bytes());
        info_body.extend_from_slice(&0u32.to_le_bytes());
        info_body.push(8);
        info_body.push(1);
        info_body.extend_from_slice(&producer.to_le_bytes());
        info_body.extend_from_slice(&0x8000u16.to_le_bytes());
        info_body.extend_from_slice(&unit_name.to_le_bytes());
        info_body.extend_from_slice(&0u32.to_le_bytes());
        let mut info = Vec::new();
        info.extend_from_slice(&(info_body.len() as u32).to_le_bytes());
        info.extend_from_slice(&info_body);

        let line = dwarf_line(code_len, &metadata.sources);
        Self {
            abbrev,
            info,
            line,
            strings,
        }
    }
}

fn dwarf_line(code_len: usize, sources: &[DebugSource]) -> Vec<u8> {
    let mut header = vec![1, 1, 1, (-5i8) as u8, 14, 13];
    header.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    header.push(0); // include directories terminator
    if sources.is_empty() {
        header.extend_from_slice(b"<aziky>\0");
        header.extend_from_slice(&[0, 0, 0]);
    } else {
        for source in sources {
            header.extend_from_slice(source.path.as_bytes());
            header.push(0);
            header.extend_from_slice(&[0, 0, 0]);
        }
    }
    header.push(0); // file table terminator
    let mut program = Vec::new();
    let file_count = sources.len().max(1);
    for file in 1..=file_count {
        if file > 1 {
            program.push(4); // DW_LNS_set_file
            push_uleb(&mut program, file as u64);
        }
        program.push(1); // DW_LNS_copy
    }
    program.push(2); // DW_LNS_advance_pc
    push_uleb(&mut program, code_len as u64);
    program.extend_from_slice(&[0, 1, 1]); // extended end_sequence
    let unit_len = 2 + 4 + header.len() + program.len();
    let mut out = Vec::new();
    out.extend_from_slice(&(unit_len as u32).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(&program);
    out
}

fn push_uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

// Mach-O MH_OBJECT support. The section set mirrors the ELF debug contract.
pub fn emit_macho64_relocatable(code: &[u8], metadata: &ArtifactMetadata) -> Vec<u8> {
    let debug = DebugSections::new(code.len(), metadata);
    let payloads = vec![
        ("__text", "__TEXT", code.to_vec(), 4u32, 0x8000_0400u32),
        (
            "__aziky",
            "__DATA",
            metadata_text(metadata).into_bytes(),
            0,
            0,
        ),
        ("__debug_abbrev", "__DWARF", debug.abbrev, 0, 0),
        ("__debug_info", "__DWARF", debug.info, 0, 0),
        ("__debug_line", "__DWARF", debug.line, 0, 0),
        ("__debug_str", "__DWARF", debug.strings, 0, 0),
    ];
    let segment_size = 72 + payloads.len() * 80;
    let command_size = segment_size + 24;
    let data_start = align_up((32 + command_size) as u64, 16) as usize;
    let mut offsets = Vec::new();
    let mut cursor = data_start;
    for (_, _, data, align, _) in &payloads {
        cursor = align_up(cursor as u64, 1u64 << align) as usize;
        offsets.push(cursor);
        cursor += data.len();
    }
    let symoff = align_up(cursor as u64, 8) as usize;
    let mut strtab = vec![0];
    let start_name = push_string(&mut strtab, "_start");
    let entry_name = push_string(&mut strtab, "_aziky_program_entry");
    let stroff = symoff + 32;

    let mut out = Vec::new();
    out.extend_from_slice(&0xfeed_facfu32.to_le_bytes());
    out.extend_from_slice(&0x0100_0007u32.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // MH_OBJECT
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(command_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0x19u32.to_le_bytes());
    out.extend_from_slice(&(segment_size as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&(cursor.saturating_sub(data_start) as u64).to_le_bytes());
    out.extend_from_slice(&(data_start as u64).to_le_bytes());
    out.extend_from_slice(&(cursor.saturating_sub(data_start) as u64).to_le_bytes());
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&7u32.to_le_bytes());
    out.extend_from_slice(&(payloads.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for ((sect, seg, data, align, flags), offset) in payloads.iter().zip(&offsets) {
        append_fixed_name(&mut out, sect);
        append_fixed_name(&mut out, seg);
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&(*offset as u32).to_le_bytes());
        out.extend_from_slice(&align.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out.extend_from_slice(&2u32.to_le_bytes()); // LC_SYMTAB
    out.extend_from_slice(&24u32.to_le_bytes());
    out.extend_from_slice(&(symoff as u32).to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(stroff as u32).to_le_bytes());
    out.extend_from_slice(&(strtab.len() as u32).to_le_bytes());
    out.resize(data_start, 0);
    for (data, offset) in payloads.iter().map(|item| &item.2).zip(offsets) {
        out.resize(offset, 0);
        out.extend_from_slice(data);
    }
    out.resize(symoff, 0);
    append_macho_symbol(&mut out, start_name);
    append_macho_symbol(&mut out, entry_name);
    out.extend_from_slice(&strtab);
    out
}

fn append_fixed_name(out: &mut Vec<u8>, name: &str) {
    assert!(name.len() <= 16);
    out.extend_from_slice(name.as_bytes());
    out.resize(out.len() + 16 - name.len(), 0);
}

fn append_macho_symbol(out: &mut Vec<u8>, name: u32) {
    out.extend_from_slice(&name.to_le_bytes());
    out.push(0x0f); // N_SECT | N_EXT
    out.push(1); // __text
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> ArtifactMetadata {
        ArtifactMetadata {
            target: "x86_64-unknown-linux".to_string(),
            sources: vec![DebugSource {
                path: "app/main.azk".to_string(),
            }],
            declarations: vec![DebugDeclaration {
                name: "main".to_string(),
                kind: "function".to_string(),
                source_index: 0,
                line: 1,
                column: 1,
                public: false,
            }],
            block_symbols: vec![(0, 0, 1)],
        }
    }

    #[test]
    fn relocatable_and_archive_magic_are_stable() {
        let object = emit_elf64_relocatable(&[0xc3], &metadata());
        assert_eq!(&object[..4], b"\x7fELF");
        assert_eq!(u16::from_le_bytes([object[16], object[17]]), 1);
        let archive = emit_static_archive(&object, &["_start"]);
        assert!(archive.starts_with(b"!<arch>\n"));
        assert_eq!(archive, emit_static_archive(&object, &["_start"]));
    }

    #[test]
    fn macho_object_has_object_filetype() {
        let object = emit_macho64_relocatable(&[0xc3], &metadata());
        assert_eq!(&object[..4], &0xfeed_facfu32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(object[12..16].try_into().unwrap()), 1);
    }

    #[test]
    fn shared_object_has_dynamic_type_and_export() {
        let shared = emit_elf64_shared(&[0xc3], &metadata());
        assert_eq!(u16::from_le_bytes([shared[16], shared[17]]), 3);
        assert!(
            shared
                .windows("aziky_program_entry".len())
                .any(|window| window == b"aziky_program_entry")
        );
    }
}
