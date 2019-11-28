//! Run a BuildLoop for `shell.nix`, watching for input file changes.
//! Can be used together with `direnv`.

use crate::build_loop::Event;
use crate::daemon::{Daemon, LoopHandlerEvent};
use crate::ops::error::{ok, ExitError, OpResult};
use crate::socket::communicate::{listener, CommunicationType};
use crate::socket::ReadWriter;
use crate::thread::Pool;
use crate::NixFile;
use crossbeam_channel as chan;
use std::collections::HashMap;

/// See the documentation for lorri::cli::Command::Shell for more
/// details.
pub fn main() -> OpResult {
    let paths = crate::ops::get_paths()?;
    let daemon_socket_file = paths.daemon_socket_file().to_owned();
    let socket_path = crate::socket::path::SocketPath::from(&daemon_socket_file);
    // TODO: move listener into Daemon struct?
    let listener = listener::Listener::new(&socket_path).map_err(|e| match e {
        crate::socket::path::BindError::OtherProcessListening => ExitError::user_error(format!(
            "Another daemon is already listening on the socket at {}. \
             We are currently only allowing one daemon to be running at the same time.",
            socket_path.display()
        )),
        e => panic!("{:?}", e),
    })?;

    let (mut daemon, build_messages_rx) = Daemon::new();

    // messages sent from accept handlers
    let (accept_messages_tx, accept_messages_rx) = chan::unbounded();

    let handlers = daemon.handlers();
    let build_events_tx = daemon.build_events_tx();

    let mut pool = Pool::new();
    pool.spawn("accept-loop", move || loop {
        let accept_messages_tx = accept_messages_tx.clone();
        // has to clone handlers once per accept loop,
        // because accept spawns a thread each time.
        let handlers = handlers.clone();
        let build_events_tx = build_events_tx.clone();
        let _handle = listener
            .accept(move |unix_stream, comm_type| match comm_type {
                CommunicationType::Ping => {
                    handlers.ping(ReadWriter::new(&unix_stream), accept_messages_tx)
                }
                CommunicationType::StreamEvents => {
                    let (tx, rx) = chan::unbounded();

                    build_events_tx
                        .send(LoopHandlerEvent::NewListener(tx))
                        .expect("daemon seems to have died");

                    handlers.stream_events(ReadWriter::new(&unix_stream), rx)
                }
            })
            // TODO
            .unwrap();
    })
    .expect("Failed to spawn accept-loop");

    pool.spawn("build-loop", || {
        let mut project_states: HashMap<NixFile, Event> = HashMap::new();
        let mut event_listeners: Vec<chan::Sender<Event>> = Vec::new();

        for msg in build_messages_rx {
            println!("{:#?}", msg);
            match &msg {
                LoopHandlerEvent::BuildEvent(ev) => match ev {
                    Event::SectionEnd => (),
                    Event::Started { nix_file, .. }
                    | Event::Completed { nix_file, .. }
                    | Event::Failure { nix_file, .. } => {
                        project_states.insert(nix_file.clone(), ev.clone());
                        event_listeners.retain(|tx| tx.send(ev.clone()).is_ok())
                    }
                },
                LoopHandlerEvent::NewListener(tx) => {
                    let keep = project_states
                        .values()
                        .all(|event| tx.send(event.clone()).is_ok());
                    if keep && tx.send(Event::SectionEnd).is_ok() {
                        event_listeners.push(tx.clone());
                    }
                }
            }
        }
    })
    .expect("Failed to spawn build-loop");

    println!("lorri: ready");

    pool.spawn("build-instruction-handler", move || {
        // For each build instruction, add the corresponding file
        // to the watch list.
        for start_build in accept_messages_rx {
            let project = crate::project::Project::new(
                start_build.nix_file,
                paths.gc_root_dir(),
                paths.cas_store().clone(),
            )
            // TODO: the project needs to create its gc root dir
            .unwrap();
            daemon.add(project)
        }
    })
    .expect("failed to spawn build-instruction-handler");

    pool.join_all_or_panic();

    ok()
}
