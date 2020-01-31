//! Uses `builder` and filesystem watch code to repeatedly
//! evaluate and build a given Nix file.

use crate::builder;
use crate::daemon::LoopHandlerEvent;
use crate::error::BuildError;
use crate::pathreduction::reduce_paths;
use crate::project::roots;
use crate::project::roots::Roots;
use crate::project::Project;
use crate::watch::{DebugMessage, EventError, Reason, Watch};
use crate::NixFile;
use crossbeam_channel as chan;
use slog_scope::{debug, warn};
use std::path::PathBuf;

/// Builder events sent back over `BuildLoop.tx`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Demarks a stream of events from recent history becoming live
    SectionEnd,
    /// A build has started
    Started {
        /// The shell.nix file for the building project
        nix_file: NixFile,
        /// The reason the build started
        reason: Reason,
    },
    /// A build completed successfully
    Completed {
        /// The shell.nix file for the building project
        nix_file: NixFile,
        /// The result of the build
        result: BuildResults,
    },
    /// A build command returned a failing exit status
    Failure {
        /// The shell.nix file for the building project
        nix_file: NixFile,
        /// The error that exited the build
        failure: BuildError,
    },
}

/// Results of a single, successful build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildResults {
    /// See `build::Info.outputPaths
    pub output_paths: builder::OutputPaths<roots::RootPath>,
}

/// The BuildLoop repeatedly builds the Nix expression in
/// `project` each time a source file influencing
/// a previous build changes.
/// Additionally, we create GC roots for the build results.
pub struct BuildLoop<'a> {
    /// Project to be built.
    project: &'a Project,
    /// Watches all input files for changes.
    /// As new input files are discovered, they are added to the watchlist.
    watch: Watch,
}

impl<'a> BuildLoop<'a> {
    /// Instatiate a new BuildLoop. Uses an internal filesystem
    /// watching implementation.
    pub fn new(project: &'a Project) -> BuildLoop<'a> {
        BuildLoop {
            project,
            watch: Watch::try_new().expect("Failed to initialize watch"),
        }
    }

    /// Loop forever, watching the filesystem for changes. Blocks.
    /// Sends `Event`s over `Self.tx` once they happen.
    /// When new filesystem changes are detected while a build is
    /// still running, it is finished first before starting a new build.
    #[allow(clippy::drop_copy, clippy::zero_ptr)] // triggered by `select!`
    pub fn forever(&mut self, tx: chan::Sender<LoopHandlerEvent>, rx_ping: chan::Receiver<()>) {
        let send = |msg| tx.send(msg).expect("Failed to send an event");
        let translate_reason = |rsn| match rsn {
            Ok(rsn) => rsn,
            // we should continue and just cite an unknown reason
            Err(EventError::EventHasNoFilePath(msg)) => {
                warn!(
                    "event has no file path; possible issue with the watcher?";
                    "message" => ?msg
                );
                // can’t Clone `Event`s, so we return the Debug output here
                Reason::UnknownEvent(DebugMessage::from(format!("{:#?}", msg)))
            }
            Err(EventError::RxNoEventReceived) => {
                panic!("The file watcher died!");
            }
        };

        // The project has just been added, so run the builder in the first iteration
        let mut reason = Some(Event::Started {
            nix_file: self.project.nix_file.clone(),
            reason: Reason::ProjectAdded(self.project.nix_file.clone()),
        });
        let mut output_paths = None;

        // Drain pings initially: we're going to trigger a first build anyway
        rx_ping.try_iter().for_each(drop);

        let rx_notify = self.watch.rx.clone();

        loop {
            // If there is some reason to build, run the build!
            if let Some(rsn) = reason {
                send(rsn.into());
                match self.once() {
                    Ok(result) => {
                        output_paths = Some(result.output_paths.clone());
                        send(
                            Event::Completed {
                                nix_file: self.project.nix_file.clone(),
                                result,
                            }
                            .into(),
                        );
                    }
                    Err(e) => {
                        if e.is_actionable() {
                            send(
                                Event::Failure {
                                    nix_file: self.project.nix_file.clone(),
                                    failure: e,
                                }
                                .into(),
                            )
                        } else {
                            panic!("Unrecoverable error:\n{:#?}", e);
                        }
                    }
                }
                reason = None;
            }

            chan::select! {
                recv(rx_notify) -> msg => if let Ok(msg) = msg {
                    if let Some(rsn) = self.watch.process(msg) {
                        reason = Some(Event::Started{
                            nix_file: self.project.nix_file.clone(),
                            reason: translate_reason(rsn)
                        });
                    }
                },
                recv(rx_ping) -> msg => if let (Ok(()), Some(output_paths)) = (msg, &output_paths) {
                    if !output_paths.shell_gc_root_is_dir() {
                        reason = Some(Event::Started{
                            nix_file: self.project.nix_file.clone(),
                            reason: Reason::PingReceived});
                    }
                },
            }
        }
    }

    /// Execute a single build of the environment.
    ///
    /// This will create GC roots and expand the file watch list for
    /// the evaluation.
    pub fn once(&mut self) -> Result<BuildResults, BuildError> {
        let run_result = builder::run(&self.project.nix_file, &self.project.cas)?;
        self.register_paths(&run_result.referenced_paths)?;
        self.root_result(run_result.result)
    }

    fn register_paths(&mut self, paths: &[PathBuf]) -> Result<(), notify::Error> {
        let original_paths_len = paths.len();
        let paths = reduce_paths(&paths);
        debug!("paths reduced"; "from" => original_paths_len, "to" => paths.len());

        // add all new (reduced) nix sources to the input source watchlist
        self.watch.extend(&paths.into_iter().collect::<Vec<_>>())?;

        Ok(())
    }

    fn root_result(&mut self, build: builder::RootedPath) -> Result<BuildResults, BuildError> {
        let roots = Roots::from_project(&self.project);

        Ok(BuildResults {
            output_paths: roots.create_roots(build).map_err(BuildError::io)?,
        })
    }
}
