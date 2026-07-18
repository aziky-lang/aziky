use super::*;

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[test]
fn macho_starts_with_magic() {
    let macho = emit_macho64_executable(&[0xC3]);
    assert_eq!(&macho[0..4], &[0xCF, 0xFA, 0xED, 0xFE]);
}

#[test]
fn code_is_placed_at_expected_offset() {
    let code = [0x90, 0xC3];
    let macho = emit_macho64_executable(&code);
    assert_eq!(
        &macho[CODE_OFFSET as usize..CODE_OFFSET as usize + code.len()],
        &code
    );
}

#[test]
fn executable_uses_modern_signable_segment_layout() {
    let code = [0x90, 0xC3];
    let macho = emit_macho64_executable(&code);
    assert_eq!(read_u32(&macho, 16), MACHO_NCMDS);
    assert_eq!(read_u32(&macho, 20), MACHO_SIZEOFCMDS);

    let pagezero = 32;
    assert_eq!(read_u32(&macho, pagezero), LOAD_CMD_SEGMENT_64);
    assert_eq!(&macho[pagezero + 8..pagezero + 18], b"__PAGEZERO");
    assert_eq!(read_u64(&macho, pagezero + 32), IMAGE_BASE);
    assert_eq!(read_u64(&macho, pagezero + 48), 0);

    let text = pagezero + LOAD_CMD_SEGMENT_64_SIZE as usize;
    assert_eq!(read_u32(&macho, text), LOAD_CMD_SEGMENT_64);
    assert_eq!(read_u32(&macho, text + 4), LOAD_CMD_TEXT_SEGMENT_SIZE);
    assert_eq!(&macho[text + 8..text + 14], b"__TEXT");
    assert_eq!(read_u32(&macho, text + 64), 1);

    let section = text + LOAD_CMD_SEGMENT_64_SIZE as usize;
    assert_eq!(&macho[section..section + 6], b"__text");
    assert_eq!(&macho[section + 16..section + 22], b"__TEXT");
    assert_eq!(read_u64(&macho, section + 32), IMAGE_BASE + CODE_OFFSET);
    assert_eq!(read_u64(&macho, section + 40), code.len() as u64);
    assert_eq!(read_u32(&macho, section + 48), CODE_OFFSET as u32);
    assert_eq!(
        read_u32(&macho, section + 64),
        S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS
    );

    let linkedit = text + LOAD_CMD_TEXT_SEGMENT_SIZE as usize;
    assert_eq!(read_u32(&macho, linkedit), LOAD_CMD_SEGMENT_64);
    assert_eq!(&macho[linkedit + 8..linkedit + 18], b"__LINKEDIT");
    let linkedit_offset = align_up(CODE_OFFSET + code.len() as u64, PAGE_SIZE);
    assert_eq!(read_u64(&macho, linkedit + 40), linkedit_offset);
    assert_eq!(read_u64(&macho, linkedit + 48), 0);
    assert_eq!(macho.len() as u64, linkedit_offset);

    let unixthread = linkedit + LOAD_CMD_SEGMENT_64_SIZE as usize;
    assert_eq!(read_u32(&macho, unixthread), LOAD_CMD_UNIXTHREAD);
    assert_eq!(read_u32(&macho, unixthread + 4), LOAD_CMD_UNIXTHREAD_SIZE);
}
