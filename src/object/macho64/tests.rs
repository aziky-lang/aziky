use super::*;

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
