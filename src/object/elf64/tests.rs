use super::*;

#[test]
fn elf_starts_with_magic() {
    let elf = emit_elf64_executable(&[0xC3]);
    assert_eq!(&elf[0..4], &[0x7F, b'E', b'L', b'F']);
}

#[test]
fn code_is_placed_at_expected_offset() {
    let code = [0x90, 0xC3];
    let elf = emit_elf64_executable(&code);
    assert_eq!(
        &elf[CODE_OFFSET as usize..CODE_OFFSET as usize + code.len()],
        &code
    );
}
