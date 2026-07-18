//! Explicit compilation targets and native runtime capability contracts.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
}

impl Architecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    Linux,
    Macos,
    Windows,
}

impl OperatingSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abi {
    Gnu,
    Darwin,
    Msvc,
}

impl Abi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gnu => "gnu",
            Self::Darwin => "darwin",
            Self::Msvc => "msvc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    Elf64,
    Macho64,
    Coff,
}

pub const WINDOWS_IMAGE_BASE: u64 = 0x1_4000_0000;
pub const WINDOWS_IAT_RVA: u64 = 0x0100_0000;

#[repr(u64)]
pub enum WindowsImport {
    ExitProcess = 0,
    GetStdHandle = 1,
    WriteFile = 2,
    ReadFile = 3,
    CloseHandle = 4,
    VirtualAlloc = 5,
    VirtualFree = 6,
    GetCurrentProcessId = 7,
    GetTickCount64 = 8,
    GetSystemTimeAsFileTime = 9,
    CreateFileA = 10,
    CreateThread = 11,
    ExitThread = 12,
    Sleep = 13,
}

impl WindowsImport {
    pub const NAMES: &'static [&'static str] = &[
        "ExitProcess",
        "GetStdHandle",
        "WriteFile",
        "ReadFile",
        "CloseHandle",
        "VirtualAlloc",
        "VirtualFree",
        "GetCurrentProcessId",
        "GetTickCount64",
        "GetSystemTimeAsFileTime",
        "CreateFileA",
        "CreateThread",
        "ExitThread",
        "Sleep",
    ];

    pub const fn address(self) -> u64 {
        WINDOWS_IMAGE_BASE + WINDOWS_IAT_RVA + self as u64 * 8
    }
}

impl ObjectFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Elf64 => "elf64",
            Self::Macho64 => "macho64",
            Self::Coff => "coff",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "elf64" => Ok(Self::Elf64),
            "macho64" => Ok(Self::Macho64),
            "coff" | "pe-coff" => Ok(Self::Coff),
            other => Err(format!(
                "unsupported format '{other}' (expected elf64, macho64, or coff)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCapabilities {
    pub startup: bool,
    pub allocation: bool,
    pub files: bool,
    pub clocks: bool,
    pub process: bool,
    pub threads: bool,
    pub synchronization: bool,
}

impl RuntimeCapabilities {
    pub const NONE: Self = Self {
        startup: false,
        allocation: false,
        files: false,
        clocks: false,
        process: false,
        threads: false,
        synchronization: false,
    };

    pub const LINUX_X86_64: Self = Self {
        startup: true,
        allocation: true,
        files: true,
        clocks: true,
        process: true,
        threads: true,
        synchronization: true,
    };

    pub fn enabled_names(self) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (enabled, name) in [
            (self.startup, "startup"),
            (self.allocation, "allocation"),
            (self.files, "files"),
            (self.clocks, "clocks"),
            (self.process, "process"),
            (self.threads, "threads"),
            (self.synchronization, "synchronization"),
        ] {
            if enabled {
                names.push(name);
            }
        }
        names
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEntryAbi {
    LinuxInitialStack,
    DarwinInitialStack,
    WindowsLoader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCallStyle {
    LinuxSyscall,
    DarwinSyscall,
    WindowsImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallNumbers {
    pub read: u64,
    pub write: u64,
    pub close: u64,
    pub mmap: u64,
    pub munmap: u64,
    pub getpid: u64,
    pub clone: u64,
    pub exit: u64,
    pub futex: u64,
    pub clock_gettime: u64,
    pub openat: u64,
    pub thread_exit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeRuntimeAbi {
    pub process_entry: ProcessEntryAbi,
    pub kernel_call_style: KernelCallStyle,
    pub syscalls: SyscallNumbers,
    pub mmap_private_anonymous: u64,
    pub prot_read_write: u64,
    pub clock_monotonic: u64,
    pub clock_realtime: u64,
    pub at_fdcwd: i64,
    pub clone_thread_flags: u64,
    pub futex_wait: u64,
    pub futex_wake: u64,
    pub mmap_no_reserve: u64,
    pub interrupted_error: i64,
    pub file_create_flags: u32,
    pub file_read_flags: u32,
}

pub const LINUX_X86_64_RUNTIME_ABI: NativeRuntimeAbi = NativeRuntimeAbi {
    process_entry: ProcessEntryAbi::LinuxInitialStack,
    kernel_call_style: KernelCallStyle::LinuxSyscall,
    syscalls: SyscallNumbers {
        read: 0,
        write: 1,
        close: 3,
        mmap: 9,
        munmap: 11,
        getpid: 39,
        clone: 56,
        exit: 60,
        futex: 202,
        clock_gettime: 228,
        openat: 257,
        thread_exit: 60,
    },
    mmap_private_anonymous: 0x22,
    prot_read_write: 0x3,
    clock_monotonic: 1,
    clock_realtime: 0,
    at_fdcwd: -100,
    clone_thread_flags: 0x350f00,
    futex_wait: 0,
    futex_wake: 1,
    mmap_no_reserve: 0x4000,
    interrupted_error: -4,
    file_create_flags: 577,
    file_read_flags: 0,
};

pub const DARWIN_X86_64_RUNTIME_ABI: NativeRuntimeAbi = NativeRuntimeAbi {
    process_entry: ProcessEntryAbi::DarwinInitialStack,
    kernel_call_style: KernelCallStyle::DarwinSyscall,
    syscalls: SyscallNumbers {
        read: 0x0200_0003,
        write: 0x0200_0004,
        close: 0x0200_0006,
        mmap: 0x0200_00c5,
        munmap: 0x0200_0049,
        getpid: 0x0200_0014,
        clone: 0,
        exit: 0x0200_0001,
        futex: 0,
        clock_gettime: 0x0200_0074,
        openat: 0x0200_01cf,
        thread_exit: 0x0200_0001,
    },
    mmap_private_anonymous: 0x1002,
    prot_read_write: 0x3,
    clock_monotonic: 6,
    clock_realtime: 0,
    at_fdcwd: -2,
    clone_thread_flags: 0,
    futex_wait: 0,
    futex_wake: 0,
    mmap_no_reserve: 0,
    interrupted_error: -4,
    file_create_flags: 0x601,
    file_read_flags: 0,
};

pub const WINDOWS_X86_64_RUNTIME_ABI: NativeRuntimeAbi = NativeRuntimeAbi {
    process_entry: ProcessEntryAbi::WindowsLoader,
    kernel_call_style: KernelCallStyle::WindowsImport,
    // Stable internal service identifiers. They deliberately mirror the Linux
    // numbers so the shared instruction emitter never needs Win32 API details.
    syscalls: SyscallNumbers {
        thread_exit: 0x1_0001,
        ..LINUX_X86_64_RUNTIME_ABI.syscalls
    },
    mmap_private_anonymous: 0x22,
    prot_read_write: 0x3,
    clock_monotonic: 1,
    clock_realtime: 0,
    at_fdcwd: -100,
    clone_thread_flags: 0,
    futex_wait: 0,
    futex_wake: 1,
    mmap_no_reserve: 0,
    interrupted_error: -4,
    file_create_flags: 577,
    file_read_flags: 0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetSpec {
    triple: &'static str,
    pub architecture: Architecture,
    pub operating_system: OperatingSystem,
    pub abi: Abi,
    pub object_format: ObjectFormat,
    pub pointer_width: u8,
    pub little_endian: bool,
    pub runtime: RuntimeCapabilities,
    native_runtime_abi: Option<NativeRuntimeAbi>,
    codegen_available: bool,
}

impl TargetSpec {
    pub const LINUX_X86_64: Self = Self {
        triple: "x86_64-unknown-linux-gnu",
        architecture: Architecture::X86_64,
        operating_system: OperatingSystem::Linux,
        abi: Abi::Gnu,
        object_format: ObjectFormat::Elf64,
        pointer_width: 64,
        little_endian: true,
        runtime: RuntimeCapabilities::LINUX_X86_64,
        native_runtime_abi: Some(LINUX_X86_64_RUNTIME_ABI),
        codegen_available: true,
    };

    pub const MACOS_X86_64: Self = Self {
        triple: "x86_64-apple-darwin",
        architecture: Architecture::X86_64,
        operating_system: OperatingSystem::Macos,
        abi: Abi::Darwin,
        object_format: ObjectFormat::Macho64,
        pointer_width: 64,
        little_endian: true,
        runtime: RuntimeCapabilities {
            startup: true,
            allocation: true,
            files: true,
            clocks: false,
            process: true,
            threads: false,
            synchronization: false,
        },
        native_runtime_abi: Some(DARWIN_X86_64_RUNTIME_ABI),
        codegen_available: true,
    };

    pub const WINDOWS_X86_64: Self = Self {
        triple: "x86_64-pc-windows-msvc",
        architecture: Architecture::X86_64,
        operating_system: OperatingSystem::Windows,
        abi: Abi::Msvc,
        object_format: ObjectFormat::Coff,
        pointer_width: 64,
        little_endian: true,
        runtime: RuntimeCapabilities {
            startup: true,
            allocation: true,
            files: true,
            clocks: true,
            process: true,
            threads: true,
            synchronization: true,
        },
        native_runtime_abi: Some(WINDOWS_X86_64_RUNTIME_ABI),
        codegen_available: true,
    };

    pub const LINUX_AARCH64: Self = Self {
        triple: "aarch64-unknown-linux-gnu",
        architecture: Architecture::Aarch64,
        operating_system: OperatingSystem::Linux,
        abi: Abi::Gnu,
        object_format: ObjectFormat::Elf64,
        pointer_width: 64,
        little_endian: true,
        runtime: RuntimeCapabilities::NONE,
        native_runtime_abi: None,
        codegen_available: false,
    };

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux" | "linux-x86_64" => {
                Ok(Self::LINUX_X86_64)
            }
            "x86_64-apple-darwin" | "macos-x86_64" => Ok(Self::MACOS_X86_64),
            "x86_64-pc-windows-msvc" | "windows-x86_64" => Ok(Self::WINDOWS_X86_64),
            "aarch64-unknown-linux-gnu" | "linux-aarch64" => Ok(Self::LINUX_AARCH64),
            other => Err(format!(
                "unsupported target '{other}' (known targets: x86_64-unknown-linux-gnu, x86_64-apple-darwin, x86_64-pc-windows-msvc, aarch64-unknown-linux-gnu)"
            )),
        }
    }

    pub fn known() -> &'static [Self] {
        &[
            Self::LINUX_AARCH64,
            Self::WINDOWS_X86_64,
            Self::MACOS_X86_64,
            Self::LINUX_X86_64,
        ]
    }

    pub fn triple(self) -> &'static str {
        self.triple
    }

    pub fn native_runtime_abi(self) -> Option<NativeRuntimeAbi> {
        self.native_runtime_abi
    }

    pub fn codegen_available(self) -> bool {
        self.codegen_available
    }

    pub fn describe(self) -> String {
        let capabilities = self.runtime.enabled_names();
        format!(
            "target={} architecture={} os={} abi={} format={} pointer-width={} endian={} codegen={} runtime={}",
            self.triple(),
            self.architecture.as_str(),
            self.operating_system.as_str(),
            self.abi.as_str(),
            self.object_format.as_str(),
            self.pointer_width,
            if self.little_endian { "little" } else { "big" },
            if self.codegen_available() {
                "accepted"
            } else {
                "planned"
            },
            if capabilities.is_empty() {
                "none".to_string()
            } else {
                capabilities.join(",")
            }
        )
    }

    pub fn require_codegen(self) -> Result<(), String> {
        if self.codegen_available {
            return Ok(());
        }
        Err(format!(
            "target '{}' is recognized but native code generation/runtime support is not accepted yet (architecture={}, os={}, abi={}, format={})",
            self.triple(),
            self.architecture.as_str(),
            self.operating_system.as_str(),
            self.abi.as_str(),
            self.object_format.as_str(),
        ))
    }

    pub fn validate_explicit_format(self, format: ObjectFormat) -> Result<(), String> {
        if format == self.object_format {
            return Ok(());
        }
        Err(format!(
            "target '{}' requires format '{}'; explicit format '{}' is incompatible",
            self.triple(),
            self.object_format.as_str(),
            format.as_str()
        ))
    }
}

impl Default for TargetSpec {
    fn default() -> Self {
        Self::LINUX_X86_64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_canonicalize_to_stable_target_triples() {
        assert_eq!(
            TargetSpec::parse("linux-x86_64").unwrap().triple(),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            TargetSpec::parse("macos-x86_64").unwrap().triple(),
            "x86_64-apple-darwin"
        );
    }

    #[test]
    fn explicit_target_rejects_an_incompatible_container() {
        let error = TargetSpec::LINUX_X86_64
            .validate_explicit_format(ObjectFormat::Macho64)
            .unwrap_err();
        assert!(error.contains("requires format 'elf64'"));
    }

    #[test]
    fn darwin_target_exposes_only_implemented_runtime_capabilities() {
        let target = TargetSpec::parse("x86_64-apple-darwin").unwrap();
        assert_eq!(target.object_format, ObjectFormat::Macho64);
        target.require_codegen().unwrap();
        assert!(target.runtime.startup);
        assert!(target.runtime.files);
        assert!(!target.runtime.clocks);
        assert!(!target.runtime.threads);
    }

    #[test]
    fn windows_target_has_the_complete_application_runtime() {
        let target = TargetSpec::parse("windows-x86_64").unwrap();
        target.require_codegen().unwrap();
        assert_eq!(target.object_format, ObjectFormat::Coff);
        assert_eq!(target.runtime, RuntimeCapabilities::LINUX_X86_64);
    }
}
