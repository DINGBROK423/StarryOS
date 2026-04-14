use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
};
use libc::ENOENT;
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);
const HELLO_DIR_ATTR: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 501,
    gid: 20,
    rdev: 0,
    blksize: 512,
    flags: 0,
};

const HELLO_TXT_CONTENT: &str = "Hello World!\n";

const HELLO_TXT_ATTR: FileAttr = FileAttr {
    ino: 2,
    size: 13,
    blocks: 1,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::RegularFile,
    perm: 0o644,
    nlink: 1,
    uid: 501,
    gid: 20,
    rdev: 0,
    blksize: 512,
    flags: 0,
};

struct HelloFS;

impl Filesystem for HelloFS {
    fn lookup(&mut self, _req: &fuser::Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent == 1 && name.to_str() == Some("hello.txt") {
            reply.entry(&TTL, &HELLO_TXT_ATTR, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    fn getattr(&mut self, _req: &fuser::Request, ino: u64, reply: ReplyAttr) {
        match ino {
            1 => reply.attr(&TTL, &HELLO_DIR_ATTR),
            2 => reply.attr(&TTL, &HELLO_TXT_ATTR),
            _ => reply.error(ENOENT),
        }
    }

    fn read(
        &mut self,
        _req: &fuser::Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        _size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        if ino == 2 {
            // 根据偏移量返回数据
            if offset >= HELLO_TXT_CONTENT.len() as i64 {
                reply.data(&[]);
            } else {
                reply.data(&HELLO_TXT_CONTENT.as_bytes()[offset as usize..]);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(
        &mut self,
        _req: &fuser::Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        let entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
            (2, FileType::RegularFile, "hello.txt"),
        ];

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 也就是下一个条目的 offset 标识
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }
}

fn main() {
    let mountpoint = "/mnt/fuse";
    
    // 确保挂载点存在 (由于处于 StarryOS 中，使用 libc::mkdir 或者 fs::create_dir_all)
    let _ = std::fs::create_dir_all(mountpoint);

    println!("Starting StarryOS FUSE test...");

    // 我们使用 libc::fork 创建子进程去读取测试，父进程执行 mount 也就是 FUSE 后台服务
    let pid = unsafe { libc::fork() };

    if pid == 0 {
        // ---- 子进程逻辑 (Client) ----
        // 等待父进程将 FUSE 挂载准备完毕
        std::thread::sleep(Duration::from_secs(2));

        println!("[Client] Testing ls:");
        let _ = std::process::Command::new("ls")
            .arg("-la")
            .arg(mountpoint)
            .status();

        println!("[Client] Testing cat:");
        let _ = std::process::Command::new("cat")
            .arg(format!("{}/hello.txt", mountpoint))
            .status();

        println!("[Client] Test finished, exiting...");
        unsafe { libc::exit(0) };
    } else if pid > 0 {
        // ---- 父进程逻辑 (Server) ----
        let options = vec![
            MountOption::RO, 
            MountOption::FSName("hello_fuse".to_string()),
            MountOption::AllowRoot,
        ];
        
        println!("[Server] Mounting FUSE to {}...", mountpoint);
        // 这里会内部调用 open("/dev/fuse") 然后通过 libc::mount 进内核，无需手工挂载
        fuser::mount2(HelloFS, mountpoint, &options).unwrap();
    } else {
        panic!("Fork failed");
    }
}
