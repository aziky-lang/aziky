use std::collections::BTreeMap;

use crate::target::{NativeRuntimeAbi, WindowsImport};

struct Stub {
    code: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    branches: Vec<(usize, &'static str)>,
}

impl Stub {
    fn new() -> Self {
        Self {
            code: Vec::new(),
            labels: BTreeMap::new(),
            branches: Vec::new(),
        }
    }

    fn label(&mut self, name: &'static str) {
        assert!(self.labels.insert(name, self.code.len()).is_none());
    }

    fn branch(&mut self, opcode: &[u8], label: &'static str) {
        self.code.extend_from_slice(opcode);
        let patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.branches.push((patch, label));
    }

    fn call_import(&mut self, import: WindowsImport) {
        self.code.extend_from_slice(&[0x49, 0xbb]); // mov r11, imm64
        self.code.extend_from_slice(&import.address().to_le_bytes());
        self.code.extend_from_slice(&[0x41, 0xff, 0x13]); // call [r11]
    }

    fn rip_label(&mut self, opcode: &[u8], label: &'static str) {
        self.code.extend_from_slice(opcode);
        let patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.branches.push((patch, label));
    }

    fn syscall_prologue(&mut self) {
        self.code.extend_from_slice(&[0x49, 0x89, 0xe3]); // mov r11,rsp
        self.code.extend_from_slice(&[0x48, 0x83, 0xe4, 0xf0]); // and rsp,-16
        self.code
            .extend_from_slice(&[0x48, 0x81, 0xec, 0x80, 0, 0, 0]); // sub rsp,128
        self.code.extend_from_slice(&[0x4c, 0x89, 0x5c, 0x24, 0x38]); // old rsp
        self.code.extend_from_slice(&[0x48, 0x89, 0x54, 0x24, 0x40]); // rdx
        self.code.extend_from_slice(&[0x4c, 0x89, 0x44, 0x24, 0x48]); // r8
        self.code.extend_from_slice(&[0x4c, 0x89, 0x4c, 0x24, 0x50]); // r9
        self.code.extend_from_slice(&[0x4c, 0x89, 0x54, 0x24, 0x58]); // r10
        self.code.extend_from_slice(&[0x48, 0x89, 0x7c, 0x24, 0x60]); // rdi
        self.code.extend_from_slice(&[0x48, 0x89, 0x74, 0x24, 0x68]); // rsi
    }

    fn syscall_return(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x40]);
        self.code.extend_from_slice(&[0x4c, 0x8b, 0x44, 0x24, 0x48]);
        self.code.extend_from_slice(&[0x4c, 0x8b, 0x4c, 0x24, 0x50]);
        self.code.extend_from_slice(&[0x4c, 0x8b, 0x54, 0x24, 0x58]);
        self.code.extend_from_slice(&[0x48, 0x8b, 0x7c, 0x24, 0x60]);
        self.code.extend_from_slice(&[0x48, 0x8b, 0x74, 0x24, 0x68]);
        self.code.extend_from_slice(&[0x48, 0x8b, 0x64, 0x24, 0x38]);
        self.code.push(0xc3);
    }

    fn finish(mut self) -> Vec<u8> {
        for (patch, label) in self.branches {
            let target = self.labels[label];
            let displacement = i32::try_from(target as i64 - (patch + 4) as i64)
                .expect("Windows runtime branch exceeds rel32");
            self.code[patch..patch + 4].copy_from_slice(&displacement.to_le_bytes());
        }
        self.code
    }
}

pub(super) fn build_dispatcher(runtime: NativeRuntimeAbi) -> Vec<u8> {
    let mut out = Stub::new();
    for (service, label) in [
        (runtime.syscalls.exit, "exit"),
        (runtime.syscalls.write, "write"),
        (runtime.syscalls.read, "read"),
        (runtime.syscalls.close, "close"),
        (runtime.syscalls.mmap, "mmap"),
        (runtime.syscalls.munmap, "munmap"),
        (runtime.syscalls.getpid, "getpid"),
        (runtime.syscalls.clock_gettime, "clock_gettime"),
        (runtime.syscalls.openat, "openat"),
        (runtime.syscalls.clone, "clone"),
        (runtime.syscalls.futex, "futex"),
        (runtime.syscalls.thread_exit, "thread_exit"),
    ] {
        out.code.extend_from_slice(&[0x48, 0x3d]); // cmp rax,imm32
        out.code.extend_from_slice(&(service as u32).to_le_bytes());
        out.branch(&[0x0f, 0x84], label);
    }
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xda, 0xff, 0xff, 0xff]); // -38
    out.code.push(0xc3);

    out.label("exit");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0x8b, 0x4c, 0x24, 0x60]); // ecx = status
    out.call_import(WindowsImport::ExitProcess);
    out.code.extend_from_slice(&[0x0f, 0x0b]); // unreachable

    out.label("write");
    out.syscall_prologue();
    out.code
        .extend_from_slice(&[0x48, 0x83, 0x7c, 0x24, 0x60, 1]);
    out.branch(&[0x0f, 0x85], "write_file_handle");
    out.code.extend_from_slice(&[0xb9, 0xf5, 0xff, 0xff, 0xff]); // STD_OUTPUT_HANDLE
    out.call_import(WindowsImport::GetStdHandle);
    out.code.extend_from_slice(&[0x48, 0x89, 0xc1]);
    out.branch(&[0xe9], "write_have_handle");
    out.label("write_file_handle");
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x60]);
    out.label("write_have_handle");
    out.code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x68]); // buffer
    out.code.extend_from_slice(&[0x4c, 0x8b, 0x44, 0x24, 0x40]); // length
    out.code.extend_from_slice(&[0x4c, 0x8d, 0x4c, 0x24, 0x70]); // written
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0x44, 0x24, 0x20, 0, 0, 0, 0]);
    out.call_import(WindowsImport::WriteFile);
    out.code.extend_from_slice(&[0x85, 0xc0]);
    out.branch(&[0x0f, 0x84], "write_failed");
    out.code.extend_from_slice(&[0x8b, 0x44, 0x24, 0x70]);
    out.branch(&[0xe9], "write_done");
    out.label("write_failed");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("write_done");
    out.syscall_return();

    out.label("read");
    out.syscall_prologue();
    out.code
        .extend_from_slice(&[0x48, 0x83, 0x7c, 0x24, 0x60, 0]);
    out.branch(&[0x0f, 0x85], "read_file_handle");
    out.code.extend_from_slice(&[0xb9, 0xf6, 0xff, 0xff, 0xff]); // STD_INPUT_HANDLE
    out.call_import(WindowsImport::GetStdHandle);
    out.code.extend_from_slice(&[0x48, 0x89, 0xc1]);
    out.branch(&[0xe9], "read_have_handle");
    out.label("read_file_handle");
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x60]);
    out.label("read_have_handle");
    out.code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x68]);
    out.code.extend_from_slice(&[0x4c, 0x8b, 0x44, 0x24, 0x40]);
    out.code.extend_from_slice(&[0x4c, 0x8d, 0x4c, 0x24, 0x70]);
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0x44, 0x24, 0x20, 0, 0, 0, 0]);
    out.call_import(WindowsImport::ReadFile);
    out.code.extend_from_slice(&[0x85, 0xc0]);
    out.branch(&[0x0f, 0x84], "read_failed");
    out.code.extend_from_slice(&[0x8b, 0x44, 0x24, 0x70]);
    out.branch(&[0xe9], "read_done");
    out.label("read_failed");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("read_done");
    out.syscall_return();

    out.label("close");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x60]);
    out.call_import(WindowsImport::CloseHandle);
    out.code.extend_from_slice(&[0x85, 0xc0]);
    out.branch(&[0x0f, 0x84], "close_failed");
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.branch(&[0xe9], "close_done");
    out.label("close_failed");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("close_done");
    out.syscall_return();

    out.label("mmap");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0x31, 0xc9]); // address = null
    out.code.extend_from_slice(&[0x48, 0x8b, 0x54, 0x24, 0x68]); // size
    out.code.extend_from_slice(&[0x41, 0xb8, 0x00, 0x30, 0, 0]);
    out.code.extend_from_slice(&[0x41, 0xb9, 0x04, 0, 0, 0]);
    out.call_import(WindowsImport::VirtualAlloc);
    out.code.extend_from_slice(&[0x48, 0x85, 0xc0]);
    out.branch(&[0x0f, 0x85], "mmap_done");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("mmap_done");
    out.syscall_return();

    out.label("munmap");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x60]);
    out.code.extend_from_slice(&[0x31, 0xd2]);
    out.code.extend_from_slice(&[0x41, 0xb8, 0x00, 0x80, 0, 0]);
    out.call_import(WindowsImport::VirtualFree);
    out.code.extend_from_slice(&[0x85, 0xc0]);
    out.branch(&[0x0f, 0x84], "munmap_failed");
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.branch(&[0xe9], "munmap_done");
    out.label("munmap_failed");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("munmap_done");
    out.syscall_return();

    out.label("getpid");
    out.syscall_prologue();
    out.call_import(WindowsImport::GetCurrentProcessId);
    out.syscall_return();

    out.label("clock_gettime");
    out.syscall_prologue();
    out.code
        .extend_from_slice(&[0x48, 0x83, 0x7c, 0x24, 0x60, 0]);
    out.branch(&[0x0f, 0x84], "clock_wall");
    out.call_import(WindowsImport::GetTickCount64);
    out.code.extend_from_slice(&[0x31, 0xd2]);
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc1, 0xe8, 0x03, 0, 0]);
    out.code.extend_from_slice(&[0x48, 0xf7, 0xf1]);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x68]);
    out.code.extend_from_slice(&[0x48, 0x89, 0x01]);
    out.code.extend_from_slice(&[0x48, 0x69, 0xd2]);
    out.code.extend_from_slice(&1_000_000_u32.to_le_bytes());
    out.code.extend_from_slice(&[0x48, 0x89, 0x51, 0x08]);
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.branch(&[0xe9], "clock_done");
    out.label("clock_wall");
    out.code.extend_from_slice(&[0x48, 0x8d, 0x4c, 0x24, 0x70]);
    out.call_import(WindowsImport::GetSystemTimeAsFileTime);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x44, 0x24, 0x70]);
    out.code.extend_from_slice(&[0x49, 0xba]);
    out.code
        .extend_from_slice(&116_444_736_000_000_000_u64.to_le_bytes());
    out.code.extend_from_slice(&[0x4c, 0x29, 0xd0]);
    out.code.extend_from_slice(&[0x31, 0xd2]);
    out.code.extend_from_slice(&[0x48, 0xb9]);
    out.code.extend_from_slice(&10_000_000_u64.to_le_bytes());
    out.code.extend_from_slice(&[0x48, 0xf7, 0xf1]);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x68]);
    out.code.extend_from_slice(&[0x48, 0x89, 0x01]);
    out.code.extend_from_slice(&[0x48, 0x6b, 0xd2, 100]);
    out.code.extend_from_slice(&[0x48, 0x89, 0x51, 0x08]);
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.label("clock_done");
    out.syscall_return();

    out.label("openat");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0x48, 0x8b, 0x4c, 0x24, 0x68]); // path
    out.code
        .extend_from_slice(&[0x48, 0x83, 0x7c, 0x24, 0x40, 0]);
    out.branch(&[0x0f, 0x84], "open_read");
    out.code.extend_from_slice(&[0xba, 0, 0, 0, 0x40]); // GENERIC_WRITE
    out.code.extend_from_slice(&[0x41, 0xba, 2, 0, 0, 0]); // CREATE_ALWAYS
    out.branch(&[0xe9], "open_common");
    out.label("open_read");
    out.code.extend_from_slice(&[0xba, 0, 0, 0, 0x80]); // GENERIC_READ
    out.code.extend_from_slice(&[0x41, 0xba, 3, 0, 0, 0]); // OPEN_EXISTING
    out.label("open_common");
    out.code.extend_from_slice(&[0x41, 0xb8, 7, 0, 0, 0]); // share all
    out.code.extend_from_slice(&[0x45, 0x31, 0xc9]);
    out.code.extend_from_slice(&[0x44, 0x89, 0x54, 0x24, 0x20]);
    out.code
        .extend_from_slice(&[0xc7, 0x44, 0x24, 0x28, 0x80, 0, 0, 0]);
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0x44, 0x24, 0x30, 0, 0, 0, 0]);
    out.call_import(WindowsImport::CreateFileA);
    out.syscall_return();

    out.label("clone");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0xc7, 0x03, 1, 0, 0, 0]);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x44, 0x24, 0x38]);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x00]);
    out.code.extend_from_slice(&[0x48, 0x89, 0x43, 0x10]);
    out.code.extend_from_slice(&[0x48, 0x8b, 0x44, 0x24, 0x68]);
    out.code.extend_from_slice(&[0x48, 0x89, 0x43, 0x18]);
    out.code.extend_from_slice(&[0x31, 0xc9, 0x31, 0xd2]);
    out.rip_label(&[0x4c, 0x8d, 0x05], "thread_entry");
    out.code.extend_from_slice(&[0x49, 0x89, 0xd9]);
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0x44, 0x24, 0x20, 0, 0, 0, 0]);
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0x44, 0x24, 0x28, 0, 0, 0, 0]);
    out.call_import(WindowsImport::CreateThread);
    out.code.extend_from_slice(&[0x48, 0x85, 0xc0]);
    out.branch(&[0x0f, 0x84], "clone_failed");
    out.code.extend_from_slice(&[0x48, 0x89, 0xc1]);
    out.call_import(WindowsImport::CloseHandle);
    out.code.extend_from_slice(&[0xb8, 1, 0, 0, 0]);
    out.branch(&[0xe9], "clone_done");
    out.label("clone_failed");
    out.code
        .extend_from_slice(&[0x48, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff]);
    out.label("clone_done");
    out.syscall_return();

    out.label("futex");
    out.syscall_prologue();
    out.code
        .extend_from_slice(&[0x48, 0x83, 0x7c, 0x24, 0x68, 0]);
    out.branch(&[0x0f, 0x84], "futex_wait");
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.branch(&[0xe9], "futex_done");
    out.label("futex_wait");
    out.code.extend_from_slice(&[0xb9, 1, 0, 0, 0]);
    out.call_import(WindowsImport::Sleep);
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.label("futex_done");
    out.syscall_return();

    out.label("thread_exit");
    out.syscall_prologue();
    out.code.extend_from_slice(&[0xc7, 0x03, 0, 0, 0, 0]);
    out.code.extend_from_slice(&[0x8b, 0x4c, 0x24, 0x60]);
    out.call_import(WindowsImport::ExitThread);
    out.code.extend_from_slice(&[0x0f, 0x0b]);

    out.label("thread_entry");
    out.code.extend_from_slice(&[0x48, 0x89, 0xcb]);
    out.code.extend_from_slice(&[0x31, 0xc0]);
    out.code.extend_from_slice(&[0xff, 0x63, 0x10]);

    out.finish()
}
