// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

/// Utilities to support Stratis.
extern crate libudev;

use std::path::Path;
use std::process::Command;

use uuid::Uuid;

use super::super::errors::{EngineError, EngineResult, ErrorEnum};

/// Common function to call a command line utility, returning an Result with an error message which
/// also includes stdout & stderr if it fails.
pub fn execute_cmd(cmd: &mut Command, error_msg: &str) -> EngineResult<()> {
    let result = cmd.output()?;
    if result.status.success() {
        Ok(())
    } else {
        let std_out_txt = String::from_utf8_lossy(&result.stdout);
        let std_err_txt = String::from_utf8_lossy(&result.stderr);
        let err_msg = format!("{} stdout: {} stderr: {}",
                              error_msg,
                              std_out_txt,
                              std_err_txt);
        Err(EngineError::Engine(ErrorEnum::Error, err_msg))
    }
}

/// Create a filesystem on devnode.
pub fn create_fs(devnode: &Path, uuid: Uuid) -> EngineResult<()> {
    execute_cmd(Command::new("mkfs.xfs")
                    .arg("-f")
                    .arg("-q")
                    .arg(&devnode)
                    .arg("-m")
                    .arg(format!("uuid={}", uuid)),
                &format!("Failed to create new filesystem at {:?}", devnode))
}

/// Use the xfs_growfs command to expand a filesystem mounted at the given
/// mount point.
pub fn xfs_growfs(mount_point: &Path) -> EngineResult<()> {
    execute_cmd(Command::new("xfs_growfs").arg(mount_point).arg("-d"),
                &format!("Failed to expand filesystem {:?}", mount_point))
}

/// Set a new UUID for filesystem on the devnode.
pub fn set_uuid(devnode: &Path, uuid: Uuid) -> EngineResult<()> {
    execute_cmd(Command::new("xfs_admin")
                    .arg("-U")
                    .arg(format!("{}", uuid))
                    .arg(&devnode),
                &format!("Failed to set UUID for filesystem {:?}", devnode))
}

/// Lookup the WWN from the udev db using the device node eg. /dev/sda
pub fn hw_lookup(dev_node_search: &Path) -> EngineResult<Option<String>> {
    #![allow(let_and_return)]
    let context = libudev::Context::new()?;
    let mut enumerator = libudev::Enumerator::new(&context)?;
    enumerator.match_subsystem("block")?;
    enumerator.match_property("DEVTYPE", "disk")?;

    let result = enumerator
        .scan_devices()?
        .find(|x| x.devnode().map_or(false, |d| dev_node_search == d))
        .map_or(Ok(None), |dev| {
            dev.property_value("ID_WWN")
                .map_or(Ok(None), |i| {
                    i.to_str()
                        .ok_or_else(|| {
                                        EngineError::Engine(ErrorEnum::Error,
                                                            format!("Unable to convert {:?} to str",
                                                                    i))
                                    })
                        .map(|i| Some(String::from(i)))
                })
        });

    result
}
