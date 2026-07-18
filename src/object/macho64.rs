const LOAD_CMD_SEGMENT_64: u32 = 0x19;
const LOAD_CMD_UNIXTHREAD: u32 = 0x5;
const LOAD_CMD_SEGMENT_64_SIZE: u32 = 72;
const X86_THREAD_STATE64: u32 = 4;
const X86_THREAD_STATE64_COUNT: u32 = 42;
const LOAD_CMD_UNIXTHREAD_SIZE: u32 = 184;
const MACHO_NCMDS: u32 = 2;
const MACHO_SIZEOFCMDS: u32 = LOAD_CMD_SEGMENT_64_SIZE + LOAD_CMD_UNIXTHREAD_SIZE;

const MH_MAGIC_64: u32 = 0xFEED_FACF;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_SUBTYPE_X86_64_ALL: u32 = 3;
const MH_EXECUTE: u32 = 2;
const MH_NOUNDEFS: u32 = 0x1;
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
    let file_size = CODE_OFFSET + code.len() as u64;
    let vm_size = align_up(file_size, PAGE_SIZE);

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

    // LC_SEGMENT_64 for a single executable __TEXT segment (no sections).
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64.to_le_bytes());
    out.extend_from_slice(&LOAD_CMD_SEGMENT_64_SIZE.to_le_bytes());
    let mut segname = [0_u8; 16];
    segname[..6].copy_from_slice(b"__TEXT");
    out.extend_from_slice(&segname);
    out.extend_from_slice(&IMAGE_BASE.to_le_bytes()); // vmaddr
    out.extend_from_slice(&vm_size.to_le_bytes()); // vmsize
    out.extend_from_slice(&0_u64.to_le_bytes()); // fileoff
    out.extend_from_slice(&file_size.to_le_bytes()); // filesize
    out.extend_from_slice(&5_u32.to_le_bytes()); // maxprot r-x
    out.extend_from_slice(&5_u32.to_le_bytes()); // initprot r-x
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
    out
}

#[cfg(test)]
#[path = "macho64/tests.rs"]
mod tests;
