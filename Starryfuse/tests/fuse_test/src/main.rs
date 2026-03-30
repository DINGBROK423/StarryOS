use std::fs::File;
use std::io::{Read, Write};
use std::mem::size_of;

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
enum FuseOpcode {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Init = 26,
    Readdir = 28,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseInHeader {
    len: u32,
    opcode: u32,
    unique: u64,
    nodeid: u64,
    uid: u32,
    gid: u32,
    pid: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseOutHeader {
    len: u32,
    error: i32,
    unique: u64,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseInitIn {
    major: u32,
    minor: u32,
    max_readahead: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseInitOut {
    major: u32,
    minor: u32,
    max_readahead: u32,
    flags: u32,
    max_background: u16,
    congestion_threshold: u16,
    max_write: u32,
    time_gran: u32,
    unused: [u32; 9],
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseAttr {
    ino: u64,
    size: u64,
    blocks: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
    atimensec: u32,
    mtimensec: u32,
    ctimensec: u32,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
    blksize: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseAttrOut {
    attr_valid: u64,
    attr_valid_nsec: u32,
    dummy: u32,
    attr: FuseAttr,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseEntryOut {
    nodeid: u64,
    generation: u64,
    entry_valid: u64,
    attr_valid: u64,
    entry_valid_nsec: u32,
    attr_valid_nsec: u32,
    attr: FuseAttr,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseDirent {
    ino: u64,
    off: u64,
    namelen: u32,
    type_: u32,
}

fn main() {
    println!("FUSE Test Daemon starting...");

    // 1. Open /dev/fuse
    let mut fuse_dev = File::options().read(true).write(true).open("/dev/fuse")
        .expect("Failed to open /dev/fuse");
    println!("Opened /dev/fuse");

    // 2. Mount /mnt/fuse
    let source = std::ffi::CString::new("none").unwrap();
    let target = std::ffi::CString::new("/mnt/fuse").unwrap();
    let fs_type = std::ffi::CString::new("fuse").unwrap();
    
    // Create mount point if not exists (kernel sys_mount should handle this, but for safety)
    let _ = std::fs::create_dir_all("/mnt/fuse");

    unsafe {
        let ret = libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fs_type.as_ptr(),
            0,
            std::ptr::null(),
        );
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            eprintln!("Failed to mount /mnt/fuse: {}", err);
            return;
        }
    }
    println!("Mounted /mnt/fuse successfully");

    // 3. Loop: Read requests and handle them
    let mut buf = [0u8; 8192];
    loop {
        let n = match fuse_dev.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now();
                continue;
            }
            Err(e) => {
                eprintln!("Error reading from /dev/fuse: {}", e);
                break;
            }
        };

        if n < size_of::<FuseInHeader>() {
            continue;
        }

        let header = unsafe { &*(buf.as_ptr() as *const FuseInHeader) };
        println!("Received FUSE request: opcode={:?}, unique={}, nodeid={}", 
            header.opcode, header.unique, header.nodeid);

        if header.opcode == FuseOpcode::Init as u32 {
            handle_init(&mut fuse_dev, header.unique);
        } else if header.opcode == FuseOpcode::Getattr as u32 {
            handle_getattr(&mut fuse_dev, header.unique, header.nodeid);
        } else if header.opcode == FuseOpcode::Lookup as u32 {
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>()..n]).unwrap_or("").trim_matches('\0');
            handle_lookup(&mut fuse_dev, header.unique, name);
        } else if header.opcode == FuseOpcode::Readdir as u32 {
            handle_readdir(&mut fuse_dev, header.unique);
        } else if header.opcode == 16 { // FuseOpcode::Write
            handle_write(&mut fuse_dev, header.unique);
        } else if header.opcode == 34 { // FuseOpcode::Create
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>() + 16 ..n]).unwrap_or("").trim_matches('\0');
            handle_create(&mut fuse_dev, header.unique, name);
        } else if header.opcode == 9 { // FuseOpcode::Mkdir
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>() + 8 ..n]).unwrap_or("").trim_matches('\0');
            handle_mkdir(&mut fuse_dev, header.unique, name);
        } else {
            // Send ENOSYS for unimplemented opcodes
            send_error(&mut fuse_dev, header.unique, 38); // ENOSYS
        }
    }
}

// ... existing helper functions ...

fn handle_create(dev: &mut File, unique: u64, _name: &str) {
    let attr = FuseAttr {
        ino: 200,
        size: 0,
        mode: 0o100644,
        nlink: 1,
        ..Default::default()
    };
    let entry_out = FuseEntryOut {
        nodeid: 200,
        attr,
        ..Default::default()
    };
    let open_out = FuseOpenOut {
        fh: 1,
        open_flags: 0,
        padding: 0,
    };
    
    let mut reply = Vec::new();
    let header = FuseOutHeader {
        len: (size_of::<FuseOutHeader>() + size_of::<FuseEntryOut>() + size_of::<FuseOpenOut>()) as u32,
        error: 0,
        unique,
    };
    reply.extend_from_slice(unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>()) });
    reply.extend_from_slice(unsafe { std::slice::from_raw_parts(&entry_out as *const _ as *const u8, size_of::<FuseEntryOut>()) });
    reply.extend_from_slice(unsafe { std::slice::from_raw_parts(&open_out as *const _ as *const u8, size_of::<FuseOpenOut>()) });
    dev.write_all(&reply).unwrap();
    println!("Sent CREATE response");
}

fn handle_mkdir(dev: &mut File, unique: u64, _name: &str) {
    let attr = FuseAttr {
        ino: 300,
        size: 4096,
        mode: 0o40755,
        nlink: 2,
        ..Default::default()
    };
    let reply = FuseEntryOut {
        nodeid: 300,
        attr,
        ..Default::default()
    };
    send_response(dev, unique, &reply);
    println!("Sent MKDIR response");
}

fn handle_write(dev: &mut File, unique: u64) {
    let reply = FuseWriteOut {
        size: 0, // Simplified
        padding: 0,
    };
    send_response(dev, unique, &reply);
    println!("Sent WRITE response");
}

#[repr(C)]
struct FuseOpenOut {
    fh: u64,
    open_flags: u32,
    padding: u32,
}

#[repr(C)]
struct FuseWriteOut {
    size: u32,
    padding: u32,
}

fn send_response<T>(dev: &mut File, unique: u64, data: &T) {
    let mut reply = Vec::new();
    let header = FuseOutHeader {
        len: (size_of::<FuseOutHeader>() + size_of::<T>()) as u32,
        error: 0,
        unique,
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>())
    };
    let data_bytes = unsafe {
        std::slice::from_raw_parts(data as *const _ as *const u8, size_of::<T>())
    };
    reply.extend_from_slice(header_bytes);
    reply.extend_from_slice(data_bytes);
    dev.write_all(&reply).unwrap();
}

fn send_error(dev: &mut File, unique: u64, error: i32) {
    let header = FuseOutHeader {
        len: size_of::<FuseOutHeader>() as u32,
        error: -error,
        unique,
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>())
    };
    dev.write_all(header_bytes).unwrap();
}

fn handle_init(dev: &mut File, unique: u64) {
    let reply = FuseInitOut {
        major: 7,
        minor: 33,
        max_readahead: 4096,
        flags: 0,
        max_write: 4096,
        ..Default::default()
    };
    send_response(dev, unique, &reply);
    println!("Sent INIT response");
}

fn handle_getattr(dev: &mut File, unique: u64, nodeid: u64) {
    let attr = if nodeid == 1 {
        // Root directory
        FuseAttr {
            ino: 1,
            size: 4096,
            mode: 0o40755,
            nlink: 2,
            ..Default::default()
        }
    } else {
        // Dummy file
        FuseAttr {
            ino: 100,
            size: 13,
            mode: 0o100644,
            nlink: 1,
            ..Default::default()
        }
    };
    let reply = FuseAttrOut {
        attr_valid: 1,
        attr,
        ..Default::default()
    };
    send_response(dev, unique, &reply);
    println!("Sent GETATTR response for nodeid={}", nodeid);
}

fn handle_lookup(dev: &mut File, unique: u64, name: &str) {
    if name == "test.txt" {
        let attr = FuseAttr {
            ino: 100,
            size: 13,
            mode: 0o100644,
            nlink: 1,
            ..Default::default()
        };
        let reply = FuseEntryOut {
            nodeid: 100,
            attr,
            ..Default::default()
        };
        send_response(dev, unique, &reply);
        println!("Sent LOOKUP response for 'test.txt'");
    } else {
        send_error(dev, unique, 2); // ENOENT
    }
}

fn handle_readdir(dev: &mut File, unique: u64) {
    // Very simplified readdir: . and test.txt
    let mut data = Vec::new();
    
    let entries = [
        (1, ".", 0o040000),
        (100, "test.txt", 0o100000),
    ];

    let mut offset = 1;
    for (ino, name, mode) in entries {
        let dirent = FuseDirent {
            ino,
            off: offset,
            namelen: name.len() as u32,
            type_: (mode >> 12) as u32,
        };
        let dirent_bytes = unsafe {
            std::slice::from_raw_parts(&dirent as *const _ as *const u8, size_of::<FuseDirent>())
        };
        data.extend_from_slice(dirent_bytes);
        data.extend_from_slice(name.as_bytes());
        // Align to 8
        while data.len() % 8 != 0 {
            data.push(0);
        }
        offset += 1;
    }

    let header = FuseOutHeader {
        len: (size_of::<FuseOutHeader>() + data.len()) as u32,
        error: 0,
        unique,
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>())
    };
    let mut reply = Vec::new();
    reply.extend_from_slice(header_bytes);
    reply.extend_from_slice(&data);
    dev.write_all(&reply).unwrap();
    println!("Sent READDIR response");
}
