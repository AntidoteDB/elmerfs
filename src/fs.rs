use crate::driver::Driver;
use crate::model::inode::Owner;
use async_std::{sync::Arc, task};
use fuse::{Filesystem, *};
use nix::{errno::Errno, libc};
use std::ffi::OsStr;
use std::path::Path;
use time::Timespec;
use tracing_futures::Instrument;

macro_rules! function {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        (&name[..name.len() - 3]).rsplit("::").next().unwrap()
    }};
}

macro_rules! check_utf8 {
    ($reply:expr, $arg:ident) => {
        match $arg.to_str() {
            Some($arg) => String::from($arg),
            None => {
                $reply.error(Errno::EINVAL as libc::c_int);
                return;
            }
        }
    };
}

macro_rules! check_name {
    ($reply:expr, $str:ident) => {{
        let n = check_utf8!($reply, $str);

        match n.parse() {
            Ok(name) => name,
            Err(_) => {
                $reply.error(Errno::EINVAL as libc::c_int);
                return;
            }
        }
    }};
}

fn ttl() -> time::Timespec {
    time::Timespec::new(0, 0)
}

macro_rules! session {
    ($req:expr, $reply:ident, $op:expr, $ok:ident => $resp:block) => {
        let unique = $req.unique();
        let (uid, gid) = ($req.uid(), $req.gid());

        let task = async move {
            let result = $op.await;

            if result.is_ok() {
                let result: Result<_, ()> = Ok(()); /* omit the content */
                tracing::debug!(?result);
            } else {
                match &result {
                    Err(crate::driver::Error::Sys(Errno::ENOENT)) => {}
                    result => {
                        tracing::error!(?result);
                    }
                }
            };

            match result {
                Ok($ok) => {
                    $resp
                }
                Err(error) => {
                    match error {
                        crate::driver::Error::Sys(errno) => {
                            $reply.error(errno as libc::c_int);
                        },
                        crate::driver::Error::Antidote(_) => {
                            $reply.error(Errno::EIO as libc::c_int);
                        }
                    }
                }
            }
        };
        let task = task.instrument(
            tracing::trace_span!("session", op = function!(), id = unique, uid, gid)
        );

        task::spawn(task);
    };

    ($req:expr, $reply:ident, $op:expr, _ => $resp:block) => {
        session!($req, $reply, $op, _r => $resp);
    };
}

pub struct Elmerfs {
    pub(crate) driver: Arc<Driver>,
}

impl Filesystem for Elmerfs {
    fn getattr(&mut self, req: &Request, ino: u64, reply: ReplyAttr) {
        let driver = self.driver.clone();

        session!(req, reply, driver.getattr(ino), attrs => {
            reply.attr(&ttl(), &attrs);
        });
    }

    fn opendir(&mut self, req: &Request, ino: u64, _flags: u32, reply: ReplyOpen) {
        let driver = self.driver.clone();

        session!(req, reply, driver.opendir(ino), _ => {
            let flags = 0;
            reply.opened(ino, flags);
        });
    }

    fn releasedir(&mut self, req: &Request, ino: u64, _fh: u64, _flags: u32, reply: ReplyEmpty) {
        let driver = self.driver.clone();

        session!(req, reply, driver.opendir(ino), _ => {
            reply.ok()
        });
    }

    fn readdir(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let driver = self.driver.clone();

        session!(req, reply, driver.readdir(ino, offset), entries => {
            for (i, entry) in entries.into_iter().enumerate() {
                let offset = offset + i as i64 + 1;

                let full = reply.add(entry.ino, offset, entry.kind, entry.name);
                if full {
                    break;
                }
            }

            reply.ok();
        });
    }

    fn lookup(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name = check_name!(reply, name);
        let driver = self.driver.clone();

        session!(req, reply, driver.lookup(parent, name), attrs => {
            let generation = 0;
            reply.entry(&ttl(), &attrs, generation);
        });
    }

    fn mkdir(
        &mut self,
        req: &Request,
        parent_ino: u64,
        name: &OsStr,
        mode: u32,
        reply: ReplyEntry,
    ) {
        let owner = Owner {
            gid: req.gid(),
            uid: req.uid(),
        };
        let name = check_name!(reply, name);
        let driver = self.driver.clone();

        session!(req, reply, driver.mkdir(owner, mode, parent_ino, name), attrs => {
            let generation = 0;
            reply.entry(&ttl(), &attrs, generation);
        });
    }

    fn rmdir(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name = check_name!(reply, name);
        let driver = self.driver.clone();

        session!(req, reply, driver.rmdir(parent, name), _ => {
            reply.ok();
        });
    }

    fn mknod(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        mode: u32,
        rdev: u32,
        reply: ReplyEntry,
    ) {
        let name = check_name!(reply, name);
        let owner = Owner {
            gid: req.gid(),
            uid: req.uid(),
        };
        let driver = self.driver.clone();

        session!(req, reply, driver.mknod(owner, mode, parent, name, rdev), attrs => {
            let generation = 0;
            reply.entry(&ttl(), &attrs, generation);
        });
    }

    fn unlink(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name = check_name!(reply, name);
        let driver = self.driver.clone();

        session!(req, reply, driver.unlink(parent, name), _ => {
            reply.ok();
        });
    }

    fn setattr(
        &mut self,
        req: &Request,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<Timespec>,
        mtime: Option<Timespec>,
        _fh: Option<u64>,
        _crtime: Option<Timespec>,
        _chgtime: Option<Timespec>,
        _bkuptime: Option<Timespec>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let t2d = |t: time::Timespec| std::time::Duration::new(t.sec as u64, t.nsec as u32);
        let atime = atime.map(t2d);
        let mtime = mtime.map(t2d);
        let driver = self.driver.clone();

        session!(
            req,
            reply,
            driver.setattr(ino, mode, uid, gid, size, atime, mtime),
            attrs => {
                reply.attr(&ttl(), &attrs);
            }
        );
    }

    fn open(&mut self, req: &Request, ino: u64, _flags: u32, reply: ReplyOpen) {
        let driver = self.driver.clone();

        session!(req, reply, driver.open(ino), _ => {
            let flags = 0;
            reply.opened(ino, flags);
        });
    }

    fn release(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let driver = self.driver.clone();

        session!(req, reply, driver.release(ino), _ => {
            reply.ok();
        });
    }

    fn write(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _flags: u32,
        reply: ReplyWrite,
    ) {
        if offset < 0 {
            reply.error(Errno::EINVAL as libc::c_int);
            return;
        }
        let offset = offset as u64;
        let driver = self.driver.clone();
        let data = Vec::from(data);

        session!(req, reply, driver.write(ino, &data, offset), _ => {
            reply.written(data.len() as u32);
        });
    }

    fn read(
        &mut self,
        req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData,
    ) {
        if offset < 0 {
            reply.error(Errno::EINVAL as libc::c_int);
            return;
        }
        let offset = offset as u64;
        let driver = self.driver.clone();

        session!(req, reply, driver.read(ino, offset, size), data => {
            reply.data(&data);
        });
    }

    fn rename(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEmpty,
    ) {
        let name = check_name!(reply, name);
        let newname = check_name!(reply, newname);
        let driver = self.driver.clone();

        session!(req, reply, driver.rename(parent, name, newparent, newname), _ => {
            reply.ok();
        });
    }

    fn link(
        &mut self,
        req: &Request,
        ino: u64,
        newparent: u64,
        newname: &OsStr,
        reply: ReplyEntry,
    ) {
        let newname = check_name!(reply, newname);
        let driver = self.driver.clone();

        session!(req, reply, driver.link(ino, newparent, newname), attrs => {
            let generation = 0;
            reply.entry(&ttl(), &attrs, generation);
        });
    }

    fn symlink(
        &mut self,
        req: &Request,
        parent: u64,
        name: &OsStr,
        link: &Path,
        reply: ReplyEntry,
    ) {
        let link = link.as_os_str();
        let link = check_utf8!(reply, link);
        let name = check_name!(reply, name);
        let owner = Owner {
            gid: req.gid(),
            uid: req.uid(),
        };
        let driver = self.driver.clone();

        session!(req, reply, driver.symlink(parent, owner, name, link), attrs => {
            let generation = 0;
            reply.entry(&ttl(), &attrs, generation);
        });
    }

    fn readlink(&mut self, req: &Request, ino: u64, reply: ReplyData) {
        let driver = self.driver.clone();

        session!(req, reply, driver.read_link(ino), path => {
            reply.data(path.as_bytes());
        });
    }
}
