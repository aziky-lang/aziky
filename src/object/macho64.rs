const LOAD_CMD_SEGMENT_64: u32 = 0x19;
const LOAD_CMD_UNIXTHREAD: u32 = 0x5;
const LOAD_CMD_SEGMENT_64_SIZE: u32 = 72;
const SECTION_64_SIZE: u32 = 80;
const LOAD_CMD_TEXT_SEGMENT_SIZE: u32 = LOAD_CMD_SEGMENT_64_SIZE + SECTION_64_SIZE;
const X86_THREAD_STATE64: u32 = 4;
const X86_THREAD_STATE64_COUNT: u32 = 42;
const LOAD_CMD_UNIXTHREAD_SIZE: u32 = 184;
const MACHO_NCMDS: u32 = 4;
const MACHO_SIZEOFCMDS: u32 = LOAD_CMD_SEGMENT_64_SIZE
    + LOAD_CMD_TEXT_SEGMENT_SIZE
    + LOAD_CMD_SEGMENT_64_SIZE
    + LOAD_CMD_UNIXTHREAD_SIZE;

const MH_MAGIC_64: u32 = 0xFEED_FACF;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_SUBTYPE_X86_64_ALL: u32 = 3;
const MH_EXECUTE: u32 = 2;
const MH_NOUNDEFS: u32 = 0x1;
const S_ATTR_PURE_INSTRUCTIONS: u32 = 0x8000_0000;
const S_ATTR_SOME_INSTRUCTIONS: u32 = 0x0000_0400;
const PAGE_SIZE: u64 = 0x1000;
const CODE_OFFSET: u64 = PAGE_SIZE;
const IMAGE_BASE: u64 = 0x1_0000_0000;

fn align_up(value: u64, align: u64) -> u64 {
    if value % align == 0 {
        value
    } else {
        value + (align - (value % align))
    }
}

pub fn emit_macho64_executable(code: &[u8]) -> Vec<u8> {
    let code_end = CODE_OFFSET + code.len() as u64;
    let linkedit_offset = align_up(code_end, PAGE_SIZE);

    let mut out = Vec::new();

    // mach_header_64
    out.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    out.extend_from_slice(&CPU_TYPE_X86_64.to_le_bytes());
    out.extend_from_slice(&CPU_SUBTYPE_X86_64_ALL.to_le_bytes());
    out.extend_from_slice(&MH_EXECUTE.to_le_bytes());
    out.extend_from_slice(&MACHO_NCMDS.to_le_bytes());
    out.extend_from_slice(&MACHO_SIZEOFCMDS.to_le_bytes());
    out.extend_from_slice(&MH_NOUNDEFS.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // reserved

    // Reserve the conventional low address range. Besides catching null and
    // truncated-pointer accesses, a page-zero segment is part of the modern
    // executable layout required by Apple's strict code-signing validator.
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64.to_le_bytes());
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64_SIZE.to_le_bytes());
    let mut segname = [0_u8; 16];
    segname[..10].copy_from_slice(b"__PAGEZERO");
    out.extend_from_slice(&segname);
    out.extend_from_slice(&0_u64.to_le_bytes()); // vmaddr
    out.extend_from_slice(&IMAGE_BASE.to_le_bytes()); // vmsize
    out.extend_from_slice(&0_u64.to_le_bytes()); // fileoff
    out.extend_from_slice(&0_u64.to_le_bytes()); // filesize
    out.extend_from_slice(&0_u32.to_le_bytes()); // maxprot
    out.extend_from_slice(&0_u32.to_le_bytes()); // initprot
    out.extend_from_slice(&0_u32.to_le_bytes()); // nsects
    out.extend_from_slice(&0_u32.to_le_bytes()); // flags

    // LC_SEGMENT_64 for __TEXT with a real executable __text section. A
    // sectionless executable segment is rejected by modern codesign strict
    // validation even though older Darwin loaders accepted it.
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64.to_le_bytes());
    out.extend_from_slice(&LOAD_CMD_TEXT_SEGMENT_SIZE.to_le_bytes());
    let mut segname = [0_u8; 16];
    segname[..6].copy_from_slice(b"__TEXT");
    out.extend_from_slice(&segname);
    out.extend_from_slice(&IMAGE_BASE.to_le_bytes()); // vmaddr
    out.extend_from_slice(&linkedit_offset.to_le_bytes()); // vmsize
    out.extend_from_slice(&0_u64.to_le_bytes()); // fileoff
    out.extend_from_slice(&linkedit_offset.to_le_bytes()); // filesize
    out.extend_from_slice(&5_u32.to_le_bytes()); // maxprot r-x
    out.extend_from_slice(&5_u32.to_le_bytes()); // initprot r-x
    out.extend_from_slice(&1_u32.to_le_bytes()); // nsects
    out.extend_from_slice(&0_u32.to_le_bytes()); // flags

    let mut sectname = [0_u8; 16];
    sectname[..6].copy_from_slice(b"__text");
    out.extend_from_slice(&sectname);
    out.extend_from_slice(&segname);
    out.extend_from_slice(&(IMAGE_BASE + CODE_OFFSET).to_le_bytes()); // addr
    out.extend_from_slice(&(code.len() as u64).to_le_bytes()); // size
    out.extend_from_slice(&(CODE_OFFSET as u32).to_le_bytes()); // offset
    out.extend_from_slice(&4_u32.to_le_bytes()); // align = 2^4
    out.extend_from_slice(&0_u32.to_le_bytes()); // reloff
    out.extend_from_slice(&0_u32.to_le_bytes()); // nreloc
    out.extend_from_slice(&(S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS).to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // reserved1
    out.extend_from_slice(&0_u32.to_le_bytes()); // reserved2
    out.extend_from_slice(&0_u32.to_le_bytes()); // reserved3

    // codesign places its ad-hoc signature payload in the final __LINKEDIT
    // segment and updates this initially empty range in place.
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64.to_le_bytes());
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64_SIZE.to_le_bytes());
    let mut linkedit_name = [0_u8; 16];
    linkedit_name[..10].copy_from_slice(b"__LINKEDIT");
    out.extend_from_slice(&linkedit_name);
    out.extend_from_slice(&(IMAGE_BASE + linkedit_offset).to_le_bytes()); // vmaddr
    out.extend_from_slice(&0_u64.to_le_bytes()); // vmsize, extended by codesign
    out.extend_from_slice(&linkedit_offset.to_le_bytes()); // fileoff
    out.extend_from_slice(&0_u64.to_le_bytes()); // filesize, extended by codesign
    out.extend_from_slice(&1_u32.to_le_bytes()); // maxprot r--
    out.extend_from_slice(&1_u32.to_le_bytes()); // initprot r--
    out.extend_from_slice(&0_u32.to_le_bytes()); // nsects
    out.extend_from_slice(&0_u32.to_le_bytes()); // flags

    // LC_UNIXTHREAD enters the self-contained image directly with the kernel's
    // initial process stack. No dyld or separately installed runtime is needed.
    out.extend_from_slice(&LOAD_CMD_UNIXTHREAD.to_le_bytes());
    out.extend_from_slice(&LOAD_CMD_UNIXTHREAD_SIZE.to_le_bytes());
    out.extend_from_slice(&X86_THREAD_STATE64.to_le_bytes());
    out.extend_from_slice(&X86_THREAD_STATE64_COUNT.to_le_bytes());
    for register in 0..21 {
        let value = if register == 16 {
            IMAGE_BASE + CODE_OFFSET // rip in x86_thread_state64_t
        } else {
            0
        };
        out.extend_from_slice(&value.to_le_bytes());
    }

    if out.len() < CODE_OFFSET as usize {
        out.resize(CODE_OFFSET as usize, 0);
    }
    out.extend_from_slice(code);
    out.resize(linkedit_offset as usize, 0);
    out
}

#[cfg(test)]
#[path = "macho64/tests.rs"]
mod tests;
