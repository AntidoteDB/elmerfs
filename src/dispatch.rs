use crate::driver::{Driver, Error};
use crate::op::{self, *};
use async_std::task;
use tracing::{self, debug, warn, error};
use std::sync::Arc;
use time;
use nix::{errno::Errno, libc};
use crate::inode::Owner;
use std::fmt::Debug;

#[tracing::instrument]
pub(super) fn drive(driver: Arc<Driver>, op_receiver: op::Receiver) {
    let ttl = || time::Timespec::new(600, 0);
   
    task::block_on(driver.configure()).unwrap();

    while let Ok(op) = op_receiver.recv() {
        debug!(?op);

        let driver = driver.clone();
        let name = op.name();
    
        // FIXME: Reply are done in the asynchronous tasks but may be blocking
        // for a significant amount of time. We should ensure that the scheduler
        // is handling this gracefully or that we explicity
        // call them inside a `spawn_blocking` block.
        match op {
            Op::GetAttr(getattr) => {
                task::spawn(async move {
                    match handle_result(name, driver.getattr(getattr.ino).await) {
                        Ok(attrs) => {
                            getattr.reply.attr(&ttl(), &attrs);
                        }
                        Err(errno) => {
                            getattr.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
            Op::Lookup(lookup) => {
                task::spawn(async move {
                    match handle_result(name, driver.lookup(lookup.parent_ino, &lookup.name).await)
                    {
                        Ok(attrs) => {
                            let generation = 0;
                            lookup.reply.entry(&ttl(), &attrs, generation);
                        }
                        Err(errno) => {
                            lookup.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
            Op::OpenDir(opendir) => {
                task::spawn(async move {
                    match handle_result(name, driver.opendir(opendir.ino).await) {
                        Ok(_) => {
                            let flags = 0;
                            opendir.reply.opened(opendir.ino, flags);
                        }
                        Err(errno) => {
                            opendir.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
            Op::ReleaseDir(releasedir) => {
                task::spawn(async move {
                    match handle_result(name, driver.releasedir(releasedir.ino).await) {
                        Ok(_) => releasedir.reply.ok(),
                        Err(errno) => {
                            releasedir.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
            Op::ReadDir(mut readdir) => {
                task::spawn(async move {
                    match handle_result(name, driver.readdir(readdir.ino, readdir.offset).await) {
                        Ok(entries) => {
                            for (i, entry) in entries.into_iter().enumerate() {
                                let offset = readdir.offset + i as i64 + 1;
    
                                let full =
                                    readdir.reply.add(entry.ino, offset, entry.kind, entry.name);
                                if full {
                                    break;
                                }
                            }
    
                            readdir.reply.ok();
                        }
                        Err(errno) => {
                            readdir.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
            Op::MkDir(mkdir) => {
                task::spawn(async move {
                    let owner = Owner {
                        gid: mkdir.gid,
                        uid: mkdir.uid,
                    };
    
                    let result = driver
                        .mkdir(owner, mkdir.mode, mkdir.parent_ino, mkdir.name)
                        .await;
                    match handle_result(name, result) {
                        Ok(attr) => {
                            let generation = 0;
                            mkdir.reply.entry(&ttl(), &attr, generation);
                        }
                        Err(errno) => {
                            mkdir.reply.error(errno as libc::c_int);
                        }
                    }
                });
            }
        }
    }
}

fn handle_result<U: Debug + Send>(name: &str, result: Result<U, Error>) -> Result<U, Errno> {
    match result {
        Ok(result) => {
            debug!(name, ?result, "success");
            Ok(result)
        }
        Err(Error::Antidote(error)) => {
            error!(name, ?error, "antidote error");
            Err(Errno::EIO)
        }
        Err(Error::Sys(errno)) => {
            warn!(name, ?errno, "system error");
            Err(errno)
        }
        Err(Error::InoAllocFailed) => {
            error!(name, "ino alloc error");
            Err(Errno::ENOSPC)
        }
    }
}