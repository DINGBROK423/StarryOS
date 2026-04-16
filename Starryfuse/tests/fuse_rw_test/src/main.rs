use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BUILD_MARKER: &str = "fuse_rw_test build 2026-04-15 rw+create+mkdir";
const RW_TEST_CONTENT: &[u8] = b"hello from rw test!";
const NEWFILE_CONTENT: &[u8] = b"new file content";

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
enum FuseOpcode {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Open = 14,
    Read = 15,
    Write = 16,
    Release = 18,
    Init = 26,
    Opendir = 27,
    Readdir = 28,
    Releasedir = 29,
    Mkdir = 9,
    Create = 35,
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

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseReadIn {
    fh: u64,
    offset: u64,
    size: u32,
    read_flags: u32,
    lock_owner: u64,
    flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseWriteIn {
    fh: u64,
    offset: u64,
    size: u32,
    write_flags: u32,
    lock_owner: u64,
    flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseCreateIn {
    flags: u32,
    mode: u32,
    umask: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseMkdirIn {
    mode: u32,
    umask: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseOpenOut {
    fh: u64,
    open_flags: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
struct FuseWriteOut {
    size: u32,
    padding: u32,
}

struct FsState {
    files: HashMap<u64, Vec<u8>>,
    dir_entries: HashMap<u64, Vec<(u64, String, u32)>>,
}

fn main() {
    println!("FUSE RW Test Daemon starting...");
    println!("{}", BUILD_MARKER);

    // 1. Open /dev/fuse
    let mut fuse_dev = File::options().read(true).write(true).open("/dev/fuse")
        .expect("Failed to open /dev/fuse");
    println!("Opened /dev/fuse");

    // Use non-blocking mode so we can exit cleanly when there are no requests.
    unsafe {
        let fd = fuse_dev.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    // 2. Mount /mnt/fuse
    let source = std::ffi::CString::new("none").unwrap();
    let target = std::ffi::CString::new("/mnt/fuse").unwrap();
    let fs_type = std::ffi::CString::new("fuse").unwrap();

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

    let state = Arc::new(Mutex::new(FsState {
        files: {
            let mut m = HashMap::new();
            m.insert(100, RW_TEST_CONTENT.to_vec());
            m
        },
        dir_entries: {
            let mut m = HashMap::new();
            m.insert(1, vec![
                (1, ".".to_string(), 0o040000),
                (1, "..".to_string(), 0o040000),
                (100, "rw_test.txt".to_string(), 0o100000),
                (300, "mydir".to_string(), 0o040000),
            ]);
            m.insert(300, vec![
                (300, ".".to_string(), 0o040000),
                (300, "..".to_string(), 0o040000),
            ]);
            m
        },
    }));

    // 3. Fork a child process that performs client operations on /mnt/fuse.
    let mut child_pid: libc::pid_t = -1;
    unsafe {
        println!("About to fork self-test child...");
        let pid = libc::fork();
        println!("fork returned {}", pid);
        if pid == 0 {
            println!("=== FUSE RW Self-Test Starting ===");

            match std::fs::read_to_string("/mnt/fuse/rw_test.txt") {
                Ok(contents) => println!("[TEST] initial read: PASS ({})", contents.trim()),
                Err(e) => println!("[TEST] initial read: FAIL ({})", e),
            }

            match File::options().write(true).truncate(true).open("/mnt/fuse/rw_test.txt") {
                Ok(mut f) => match f.write_all(NEWFILE_CONTENT) {
                    Ok(()) => println!("[TEST] write existing: PASS"),
                    Err(e) => println!("[TEST] write existing: FAIL ({})", e),
                },
                Err(e) => println!("[TEST] write existing: FAIL ({})", e),
            }

            match std::fs::read_to_string("/mnt/fuse/rw_test.txt") {
                Ok(contents) => {
                    let expected = std::str::from_utf8(NEWFILE_CONTENT).unwrap();
                    if contents.trim() == expected {
                        println!("[TEST] read-back: PASS ({})", contents.trim());
                    } else {
                        println!("[TEST] read-back: MISMATCH (expected {:?}, got {:?})", expected, contents.trim());
                    }
                }
                Err(e) => println!("[TEST] read-back: FAIL ({})", e),
            }

            match std::fs::create_dir("/mnt/fuse/mydir") {
                Ok(()) => println!("[TEST] mkdir: PASS"),
                Err(e) => println!("[TEST] mkdir: FAIL ({})", e),
            }

            match File::options().write(true).create(true).truncate(true).open("/mnt/fuse/newfile.txt") {
                Ok(mut f) => match f.write_all(NEWFILE_CONTENT) {
                    Ok(()) => println!("[TEST] create+write: PASS"),
                    Err(e) => println!("[TEST] create+write: FAIL ({})", e),
                },
                Err(e) => println!("[TEST] create+write: FAIL ({})", e),
            }

            match std::fs::read_to_string("/mnt/fuse/newfile.txt") {
                Ok(contents) => {
                    let expected = std::str::from_utf8(NEWFILE_CONTENT).unwrap();
                    if contents.trim() == expected {
                        println!("[TEST] read newfile: PASS ({})", contents.trim());
                    } else {
                        println!("[TEST] read newfile: MISMATCH (expected {:?}, got {:?})", expected, contents.trim());
                    }
                }
                Err(e) => println!("[TEST] read newfile: FAIL ({})", e),
            }

            match std::fs::read_dir("/mnt/fuse") {
                Ok(entries) => {
                    let names: Vec<String> = entries
                        .filter_map(|e| e.ok().map(|x| x.file_name().to_string_lossy().to_string()))
                        .collect();
                    let entries_str = names.join(",");
                    println!("[TEST] readdir: entries={}", entries_str);
                    if names.iter().any(|n| n == "mydir") && names.iter().any(|n| n == "newfile.txt") && names.iter().any(|n| n == "rw_test.txt") {
                        println!("[TEST] readdir: PASS");
                    } else {
                        println!("[TEST] readdir: PARTIAL (missing entries)");
                    }
                }
                Err(e) => println!("[TEST] readdir: FAIL ({})", e),
            }

            match std::fs::read_dir("/mnt/fuse/mydir") {
                Ok(entries) => {
                    let names: Vec<String> = entries
                        .filter_map(|e| e.ok().map(|x| x.file_name().to_string_lossy().to_string()))
                        .collect();
                    let entries_str = names.join(",");
                    println!("[TEST] readdir mydir: entries={}", entries_str);
                    println!("[TEST] readdir mydir: PASS");
                }
                Err(e) => println!("[TEST] readdir mydir: FAIL ({})", e),
            }

            println!("=== FUSE RW Self-Test Complete ===");
            libc::_exit(0);
        } else if pid > 0 {
            child_pid = pid;
            println!("Spawned self-test child pid={}", child_pid);
        } else {
            eprintln!(
                "fork failed: {}, please test /mnt/fuse from another process",
                std::io::Error::last_os_error()
            );
        }
    }

    // 4. Loop: Read requests and handle them, exit after child exits and idle period.
    let mut buf = [0u8; 8192];
    let mut child_done = child_pid <= 0;
    let mut child_done_since = if child_done {
        Some(Instant::now())
    } else {
        None
    };
    let fuse_fd = fuse_dev.as_raw_fd();

    loop {
        if child_pid > 0 {
            let mut status: libc::c_int = 0;
            let ret = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
            if ret == child_pid {
                child_done = true;
                child_pid = -1;
                child_done_since = Some(Instant::now());
                println!("Self-test child exited, status={}", status);
            } else if ret < 0 {
                child_done = true;
                child_pid = -1;
                child_done_since = Some(Instant::now());
                eprintln!("waitpid failed: {}", std::io::Error::last_os_error());
            }
        }

        let mut pfd = libc::pollfd {
            fd: fuse_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let poll_ret = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, 20) };
        if poll_ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("poll error: {}", err);
            break;
        }

        if poll_ret == 0 || (pfd.revents & libc::POLLIN) == 0 {
            if child_done
                && child_done_since
                    .map(|t| t.elapsed() >= Duration::from_millis(500))
                    .unwrap_or(false)
            {
                println!("Test complete, daemon exiting.");
                break;
            }
            continue;
        }

        let n = match fuse_dev.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                eprintln!("Error reading from /dev/fuse: {}", e);
                break;
            }
        };

        if n < size_of::<FuseInHeader>() {
            continue;
        }

        let header = unsafe { &*(buf.as_ptr() as *const FuseInHeader) };
        println!("Received FUSE request: opcode={}, unique={}, nodeid={}",
            header.opcode, header.unique, header.nodeid);

        if header.opcode == FuseOpcode::Init as u32 {
            handle_init(&mut fuse_dev, header.unique);
        } else if header.opcode == FuseOpcode::Getattr as u32 {
            handle_getattr(&mut fuse_dev, header.unique, header.nodeid, &state);
        } else if header.opcode == FuseOpcode::Lookup as u32 {
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>()..n]).unwrap_or("").trim_matches('\0');
            handle_lookup(&mut fuse_dev, header.unique, name, header.nodeid, &state);
        } else if header.opcode == FuseOpcode::Open as u32 {
            handle_open(&mut fuse_dev, header.unique, header.nodeid, &state);
        } else if header.opcode == FuseOpcode::Read as u32 {
            let mut req_offset = 0u64;
            let mut req_size = 4096usize;
            if n >= size_of::<FuseInHeader>() + size_of::<FuseReadIn>() {
                let read_in = unsafe {
                    &*(buf[size_of::<FuseInHeader>()..].as_ptr() as *const FuseReadIn)
                };
                req_offset = read_in.offset;
                req_size = read_in.size as usize;
            }
            handle_read(&mut fuse_dev, header.unique, header.nodeid, req_offset, req_size, &state);
        } else if header.opcode == FuseOpcode::Release as u32 {
            handle_release(&mut fuse_dev, header.unique, false);
        } else if header.opcode == FuseOpcode::Opendir as u32 {
            handle_open(&mut fuse_dev, header.unique, header.nodeid, &state);
        } else if header.opcode == FuseOpcode::Readdir as u32 {
            let mut req_offset = 0u64;
            if n >= size_of::<FuseInHeader>() + size_of::<FuseReadIn>() {
                let read_in = unsafe {
                    &*(buf[size_of::<FuseInHeader>()..].as_ptr() as *const FuseReadIn)
                };
                req_offset = read_in.offset;
            }
            handle_readdir(&mut fuse_dev, header.unique, header.nodeid, req_offset, &state);
        } else if header.opcode == FuseOpcode::Releasedir as u32 {
            handle_release(&mut fuse_dev, header.unique, true);
        } else if header.opcode == FuseOpcode::Write as u32 {
            let write_in = if n >= size_of::<FuseInHeader>() + size_of::<FuseWriteIn>() {
                unsafe { &*(buf[size_of::<FuseInHeader>()..].as_ptr() as *const FuseWriteIn) }
            } else {
                &FuseWriteIn::default()
            };
            let data_start = size_of::<FuseInHeader>() + size_of::<FuseWriteIn>();
            let data = if n > data_start {
                &buf[data_start..n.min(data_start + write_in.size as usize)]
            } else {
                &[]
            };
            handle_write(&mut fuse_dev, header.unique, header.nodeid, write_in.offset, data, &state);
        } else if header.opcode == FuseOpcode::Mkdir as u32 {
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>() + size_of::<FuseMkdirIn>()..n])
                .unwrap_or("").trim_matches('\0');
            handle_mkdir(&mut fuse_dev, header.unique, name, &state);
        } else if header.opcode == FuseOpcode::Create as u32 {
            let name = std::str::from_utf8(&buf[size_of::<FuseInHeader>() + size_of::<FuseCreateIn>()..n])
                .unwrap_or("").trim_matches('\0');
            handle_create(&mut fuse_dev, header.unique, name, &state);
        } else if header.opcode == 4 { // FuseOpcode::Setattr
            handle_setattr(&mut fuse_dev, header.unique, header.nodeid, &state);
        } else {
            // Send ENOSYS for unimplemented opcodes
            send_error(&mut fuse_dev, header.unique, 38); // ENOSYS
        }
    }
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

fn handle_getattr(dev: &mut File, unique: u64, nodeid: u64, state: &Arc<Mutex<FsState>>) {
    let state = state.lock().unwrap();
    let attr = if nodeid == 1 {
        FuseAttr {
            ino: 1,
            size: 4096,
            mode: 0o40755,
            nlink: 2,
            ..Default::default()
        }
    } else if nodeid == 300 {
        FuseAttr {
            ino: 300,
            size: 4096,
            mode: 0o40755,
            nlink: 2,
            ..Default::default()
        }
    } else if let Some(content) = state.files.get(&nodeid) {
        FuseAttr {
            ino: nodeid,
            size: content.len() as u64,
            mode: 0o100644,
            nlink: 1,
            ..Default::default()
        }
    } else {
        drop(state);
        send_error(dev, unique, 2); // ENOENT
        return;
    };
    let reply = FuseAttrOut {
        attr_valid: 1,
        attr,
        ..Default::default()
    };
    drop(state);
    send_response(dev, unique, &reply);
    println!("Sent GETATTR response for nodeid={}", nodeid);
}

fn handle_lookup(dev: &mut File, unique: u64, name: &str, nodeid: u64, state: &Arc<Mutex<FsState>>) {
    let state = state.lock().unwrap();
    if let Some(entries) = state.dir_entries.get(&nodeid) {
        for (ino, entry_name, mode) in entries {
            if entry_name == name {
                let size = if *ino == 300 {
                    4096
                } else {
                    state.files.get(ino).map(|v| v.len() as u64).unwrap_or(0)
                };
                let attr = FuseAttr {
                    ino: *ino,
                    size,
                    mode: *mode as u32,
                    nlink: if *mode == 0o040000 { 2 } else { 1 },
                    ..Default::default()
                };
                let reply = FuseEntryOut {
                    nodeid: *ino,
                    attr,
                    ..Default::default()
                };
                drop(state);
                send_response(dev, unique, &reply);
                println!("Sent LOOKUP response for '{}'", name);
                return;
            }
        }
    }
    drop(state);
    send_error(dev, unique, 2); // ENOENT
}

fn handle_open(dev: &mut File, unique: u64, nodeid: u64, _state: &Arc<Mutex<FsState>>) {
    if nodeid != 1 && nodeid != 100 && nodeid != 200 && nodeid != 300 {
        send_error(dev, unique, 2); // ENOENT
        return;
    }

    let reply = FuseOpenOut {
        fh: nodeid,
        open_flags: 0,
        padding: 0,
    };
    send_response(dev, unique, &reply);
    println!("Sent OPEN response for nodeid={}", nodeid);
}

fn handle_read(dev: &mut File, unique: u64, nodeid: u64, req_offset: u64, req_size: usize, state: &Arc<Mutex<FsState>>) {
    let payload: Vec<u8> = {
        let state = state.lock().unwrap();
        let content = match state.files.get(&nodeid) {
            Some(c) => c,
            None => {
                drop(state);
                send_error(dev, unique, 2); // ENOENT
                return;
            }
        };
        let start = (req_offset as usize).min(content.len());
        let end = start.saturating_add(req_size).min(content.len());
        content[start..end].to_vec()
    };

    let header = FuseOutHeader {
        len: (size_of::<FuseOutHeader>() + payload.len()) as u32,
        error: 0,
        unique,
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>())
    };

    let mut reply = Vec::new();
    reply.extend_from_slice(header_bytes);
    reply.extend_from_slice(&payload);
    dev.write_all(&reply).unwrap();
    println!(
        "Sent READ response (nodeid={}, offset={}, req_size={}, bytes={})",
        nodeid,
        req_offset,
        req_size,
        payload.len()
    );
}

fn handle_write(dev: &mut File, unique: u64, nodeid: u64, offset: u64, data: &[u8], state: &Arc<Mutex<FsState>>) {
    let mut state = state.lock().unwrap();
    if let Some(content) = state.files.get_mut(&nodeid) {
        let off = offset as usize;
        if off > content.len() {
            content.resize(off, 0);
        }
        let end = off + data.len();
        if end > content.len() {
            content.resize(end, 0);
        }
        content[off..end].copy_from_slice(data);
        let written = data.len() as u32;
        let reply = FuseWriteOut { size: written, padding: 0 };
        drop(state);
        send_response(dev, unique, &reply);
        println!("Sent WRITE response (nodeid={}, offset={}, bytes={})", nodeid, offset, written);
    } else {
        drop(state);
        send_error(dev, unique, 2); // ENOENT
    }
}

fn handle_mkdir(dev: &mut File, unique: u64, _name: &str, _state: &Arc<Mutex<FsState>>) {
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

fn handle_setattr(dev: &mut File, unique: u64, nodeid: u64, state: &Arc<Mutex<FsState>>) {
    let mut st = state.lock().unwrap();
    if let Some(content) = st.files.get_mut(&nodeid) {
        content.clear();
    }
    let size = st.files.get(&nodeid).map(|c| c.len() as u64).unwrap_or(0);
    drop(st);
    let attr = FuseAttr {
        ino: nodeid,
        size,
        mode: 0o100644,
        nlink: 1,
        ..Default::default()
    };
    let reply = FuseAttrOut {
        attr_valid: 1,
        attr,
        ..Default::default()
    };
    send_response(dev, unique, &reply);
    println!("Sent SETATTR response for nodeid={}", nodeid);
}

fn handle_create(dev: &mut File, unique: u64, name: &str, state: &Arc<Mutex<FsState>>) {
    let mut state = state.lock().unwrap();
    state.files.insert(200, Vec::new());
    if let Some(entries) = state.dir_entries.get_mut(&1) {
        if !entries.iter().any(|(_, n, _)| n == name) {
            entries.push((200, name.to_string(), 0o100000));
        }
    }
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
        fh: 200,
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
    drop(state);
    dev.write_all(&reply).unwrap();
    println!("Sent CREATE response for '{}'", name);
}

fn handle_readdir(dev: &mut File, unique: u64, nodeid: u64, req_offset: u64, state: &Arc<Mutex<FsState>>) {
    let state = state.lock().unwrap();
    let mut data = Vec::new();

    if req_offset == 0 {
        let entries = state.dir_entries.get(&nodeid).cloned().unwrap_or_default();
        let mut next_off = 1u64;
        for (ino, name, mode) in entries {
            let dirent = FuseDirent {
                ino,
                off: next_off,
                namelen: name.len() as u32,
                type_: (mode >> 12) as u32,
            };
            let dirent_bytes = unsafe {
                std::slice::from_raw_parts(&dirent as *const _ as *const u8, size_of::<FuseDirent>())
            };
            data.extend_from_slice(dirent_bytes);
            data.extend_from_slice(name.as_bytes());
            while data.len() % 8 != 0 {
                data.push(0);
            }
            next_off += 1;
        }
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
    drop(state);
    dev.write_all(&reply).unwrap();
    println!("Sent READDIR response (offset={}, bytes={})", req_offset, data.len());
}

fn handle_release(dev: &mut File, unique: u64, is_dir: bool) {
    let header = FuseOutHeader {
        len: size_of::<FuseOutHeader>() as u32,
        error: 0,
        unique,
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<FuseOutHeader>())
    };
    dev.write_all(header_bytes).unwrap();
    if is_dir {
        println!("Sent RELEASEDIR response");
    } else {
        println!("Sent RELEASE response");
    }
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
