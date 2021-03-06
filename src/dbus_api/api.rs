// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use std::path::Path;
use std::vec::Vec;
use std::rc::Rc;
use std::cell::RefCell;

use dbus;
use dbus::Connection;
use dbus::BusType;
use dbus::Message;
use dbus::NameFlag;
use dbus::arg::Array;
use dbus::arg::IterAppend;
use dbus::tree::Access;
use dbus::tree::EmitsChangedSignal;
use dbus::tree::Factory;
use dbus::tree::MethodErr;
use dbus::tree::MTFn;
use dbus::tree::MethodResult;
use dbus::tree::MethodInfo;
use dbus::tree::PropInfo;
use dbus::tree::Tree;
use dbus::ConnectionItem;

use engine::Engine;
use stratis::VERSION;

use super::filesystem::create_dbus_filesystem;
use super::blockdev::create_dbus_blockdev;
use super::pool::create_dbus_pool;
use super::types::{ActionQueue, DeferredAction, DbusContext, DbusErrorEnum, TData};
use super::util::STRATIS_BASE_PATH;
use super::util::STRATIS_BASE_SERVICE;
use super::util::engine_to_dbus_err_tuple;
use super::util::get_next_arg;
use super::util::msg_code_ok;
use super::util::msg_string_ok;
use super::util::tuple_to_option;

fn create_pool(m: &MethodInfo<MTFn<TData>, TData>) -> MethodResult {
    let message: &Message = m.msg;
    let mut iter = message.iter_init();

    let name: &str = get_next_arg(&mut iter, 0)?;
    let redundancy: (bool, u16) = get_next_arg(&mut iter, 1)?;
    let force: bool = get_next_arg(&mut iter, 2)?;
    let devs: Array<&str, _> = get_next_arg(&mut iter, 3)?;

    let blockdevs = devs.map(|x| Path::new(x)).collect::<Vec<&Path>>();

    let object_path = m.path.get_name();
    let dbus_context = m.tree.get_data();
    let mut engine = dbus_context.engine.borrow_mut();
    let result = engine.create_pool(name, &blockdevs, tuple_to_option(redundancy), force);

    let return_message = message.method_return();

    let default_return: (dbus::Path, Vec<dbus::Path>) = (dbus::Path::default(), Vec::new());

    let msg = match result {
        Ok(pool_uuid) => {
            let pool_object_path: dbus::Path =
                create_dbus_pool(dbus_context, object_path.clone(), pool_uuid);

            let pool = get_mut_pool!(engine; pool_uuid; default_return; return_message);

            let bd_object_paths = pool.blockdevs()
                .iter()
                .map(|bd| create_dbus_blockdev(dbus_context, pool_object_path.clone(), bd.uuid()))
                .collect::<Vec<_>>();

            return_message.append3((pool_object_path, bd_object_paths),
                                   msg_code_ok(),
                                   msg_string_ok())
        }
        Err(x) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&x);
            return_message.append3(default_return, rc, rs)
        }
    };
    Ok(vec![msg])
}

fn destroy_pool(m: &MethodInfo<MTFn<TData>, TData>) -> MethodResult {

    let message: &Message = m.msg;
    let mut iter = message.iter_init();

    let object_path: dbus::Path<'static> = get_next_arg(&mut iter, 0)?;

    let dbus_context = m.tree.get_data();

    let default_return = false;
    let return_message = message.method_return();

    let pool_uuid = match m.tree.get(&object_path) {
        Some(pool_path) => get_data!(pool_path; default_return; return_message).uuid,
        None => {
            return Ok(vec![return_message.append3(default_return, msg_code_ok(), msg_string_ok())]);
        }
    };

    let msg = match dbus_context.engine.borrow_mut().destroy_pool(pool_uuid) {
        Ok(action) => {
            dbus_context
                .actions
                .borrow_mut()
                .push_remove(object_path);
            return_message.append3(action, msg_code_ok(), msg_string_ok())
        }
        Err(err) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&err);
            return_message.append3(default_return, rc, rs)
        }
    };
    Ok(vec![msg])
}

fn get_version(i: &mut IterAppend, _p: &PropInfo<MTFn<TData>, TData>) -> Result<(), MethodErr> {
    i.append(VERSION);
    Ok(())
}

fn configure_simulator(m: &MethodInfo<MTFn<TData>, TData>) -> MethodResult {
    let message = m.msg;
    let mut iter = message.iter_init();

    let denominator: u32 = get_next_arg(&mut iter, 0)?;

    let dbus_context = m.tree.get_data();
    let result = dbus_context
        .engine
        .borrow_mut()
        .configure_simulator(denominator);

    let return_message = message.method_return();

    let msg = match result {
        Ok(_) => return_message.append2(msg_code_ok(), msg_string_ok()),
        Err(err) => {
            let (rc, rs) = engine_to_dbus_err_tuple(&err);
            return_message.append2(rc, rs)
        }
    };
    Ok(vec![msg])
}

fn get_base_tree<'a>(dbus_context: DbusContext) -> (Tree<MTFn<TData>, TData>, dbus::Path<'a>) {

    let f = Factory::new_fn();

    let base_tree = f.tree(dbus_context);

    let create_pool_method = f.method("CreatePool", (), create_pool)
        .in_arg(("name", "s"))
        .in_arg(("redundancy", "(bq)"))
        .in_arg(("force", "b"))
        .in_arg(("devices", "as"))
        .out_arg(("result", "(oao)"))
        .out_arg(("return_code", "q"))
        .out_arg(("return_string", "s"));

    let destroy_pool_method = f.method("DestroyPool", (), destroy_pool)
        .in_arg(("pool", "o"))
        .out_arg(("action", "b"))
        .out_arg(("return_code", "q"))
        .out_arg(("return_string", "s"));

    let configure_simulator_method = f.method("ConfigureSimulator", (), configure_simulator)
        .in_arg(("denominator", "u"))
        .out_arg(("return_code", "q"))
        .out_arg(("return_string", "s"));

    let version_property = f.property::<&str, _>("Version", ())
        .access(Access::Read)
        .emits_changed(EmitsChangedSignal::Const)
        .on_get(get_version);

    let interface_name = format!("{}.{}", STRATIS_BASE_SERVICE, "Manager");

    let obj_path = f.object_path(STRATIS_BASE_PATH, None)
        .introspectable()
        .object_manager()
        .add(f.interface(interface_name, ())
                 .add_m(create_pool_method)
                 .add_m(destroy_pool_method)
                 .add_m(configure_simulator_method)
                 .add_p(version_property));

    let path = obj_path.get_name().to_owned();
    (base_tree.add(obj_path), path)
}

#[allow(type_complexity)]
pub fn connect(engine: Rc<RefCell<Engine>>)
               -> Result<(Connection, Tree<MTFn<TData>, TData>, DbusContext), dbus::Error> {
    let c = Connection::get_private(BusType::System)?;

    let local_engine = Rc::clone(&engine);

    let (mut tree, object_path) = get_base_tree(DbusContext::new(engine));
    let dbus_context = tree.get_data().clone();

    // This should never panic as create_dbus_pool(),
    // create_dbus_filesystem(), and create_dbus_blockdev() do not borrow the
    // engine.
    for pool in local_engine.borrow().pools() {
        let pool_path = create_dbus_pool(&dbus_context, object_path.clone(), pool.uuid());
        for fs_uuid in pool.filesystems().iter().map(|f| f.uuid()) {
            create_dbus_filesystem(&dbus_context, pool_path.clone(), fs_uuid);
        }
        for dev_uuid in pool.blockdevs().iter().map(|bd| bd.uuid()) {
            create_dbus_blockdev(&dbus_context, pool_path.clone(), dev_uuid);
        }
    }

    tree.set_registered(&c, true)?;

    c.register_name(STRATIS_BASE_SERVICE, NameFlag::ReplaceExisting as u32)?;

    process_deferred_actions(&c, &mut tree, &mut dbus_context.actions.borrow_mut())?;

    Ok((c, tree, dbus_context))
}

/// Update the dbus tree with deferred adds and removes.
fn process_deferred_actions(c: &Connection,
                            tree: &mut Tree<MTFn<TData>, TData>,
                            actions: &mut ActionQueue)
                            -> Result<(), dbus::Error> {
    for action in actions.drain() {
        match action {
            DeferredAction::Add(path) => {
                c.register_object_path(path.get_name())?;
                tree.insert(path);
            }
            DeferredAction::Remove(path) => {
                c.unregister_object_path(&path);
                tree.remove(&path);
            }
        }
    }
    Ok(())
}

pub fn handle(c: &Connection,
              item: &ConnectionItem,
              tree: &mut Tree<MTFn<TData>, TData>,
              dbus_context: &DbusContext)
              -> Result<(), dbus::Error> {
    if let ConnectionItem::MethodCall(ref msg) = *item {
        if let Some(v) = tree.handle(msg) {
            // Probably the wisest is to ignore any send errors here -
            // maybe the remote has disconnected during our processing.
            for m in v {
                let _ = c.send(m);
            }
        }

        process_deferred_actions(c, tree, &mut dbus_context.actions.borrow_mut())?;
    }

    Ok(())
}
