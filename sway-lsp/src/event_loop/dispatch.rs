//! See [RequestDispatcher].
use lsp_server::ExtractError;
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, panic, thread};
use stdx::thread::ThreadIntent;

use crate::{
    error::LanguageServerError,
    event_loop::{
        self,
        main_loop::Task,
        server_state_ext::{ServerStateExt, ServerStateSnapshot},
        Cancelled, LspError,
    },
    server_state::ServerState,
};

/// A visitor for routing a raw JSON request to an appropriate handler function.
///
/// Most requests are read-only and async and are handled on the threadpool
/// (`on` method).
///
/// Some read-only requests are latency sensitive, and are immediately handled
/// on the main loop thread (`on_sync`). These are typically typing-related
/// requests.
///
/// Some requests modify the state, and are run on the main thread to get
/// `&mut` (`on_sync_mut`).
///
/// Read-only requests are wrapped into `catch_unwind` -- they don't modify the
/// state, so it's OK to recover from their failures.
pub(crate) struct RequestDispatcher<'a> {
    pub(crate) req: Option<lsp_server::Request>,
    pub(crate) server_state: &'a mut ServerStateExt,
}

impl<'a> RequestDispatcher<'a> {
    /// Dispatches the request onto the current thread, given full access to
    /// mutable global state. Unlike all other methods here, this one isn't
    /// guarded by `catch_unwind`, so, please, don't make bugs :-)
    pub(crate) fn on_sync_mut<R>(
        &mut self,
        f: fn(&mut ServerStateExt, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request,
        R::Params: DeserializeOwned + panic::UnwindSafe + fmt::Debug,
        R::Result: Serialize,
    {
        let (req, params, panic_context) = match self.parse::<R>() {
            Some(it) => it,
            None => return self,
        };
        let result = {
            let _pctx = stdx::panic_context::enter(panic_context);
            f(self.server_state, params)
        };
        if let Ok(response) = result_to_response::<R>(req.id, result) {
            self.server_state.respond(response);
        }

        self
    }

    /// Dispatches the request onto the current thread.
    pub(crate) fn on_sync<R>(
        &mut self,
        f: fn(ServerStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request,
        R::Params: DeserializeOwned + panic::UnwindSafe + fmt::Debug,
        R::Result: Serialize,
    {
        let (req, params, panic_context) = match self.parse::<R>() {
            Some(it) => it,
            None => return self,
        };
        let global_state_snapshot = self.server_state.snapshot();

        // Note, RA is doing this correctly, we just cant atm because the catch_unwind doesn't
        // allow inner types to have interior mutability, which DashMap does
        // let result = panic::catch_unwind(move || {
        //     let _pctx = stdx::panic_context::enter(panic_context);
        //     f(global_state_snapshot, params)
        // });
        //
        // if let Ok(response) = thread_result_to_response::<R>(req.id, result) {
        //     self.server_state.respond(response);
        // }

        let result = {
            let _pctx = stdx::panic_context::enter(panic_context);
            f(global_state_snapshot, params)
        };
        if let Ok(response) = result_to_response::<R>(req.id, result) {
            self.server_state.respond(response);
        }

        self
    }

    /// Dispatches a non-latency-sensitive request onto the thread pool
    /// without retrying it if it panics.
    pub(crate) fn on_no_retry<R>(
        &mut self,
        f: fn(ServerStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request + 'static,
        R::Params: DeserializeOwned + panic::UnwindSafe + Send + fmt::Debug,
        R::Result: Serialize,
    {
        let (req, params, panic_context) = match self.parse::<R>() {
            Some(it) => it,
            None => return self,
        };

        self.server_state
            .event_loop_state
            .task_pool
            .handle
            .spawn(ThreadIntent::Worker, {
                let world = self.server_state.snapshot();
                move || {
                    // Note, RA is doing this correctly, we just cant atm because the catch_unwind doesn't
                    // allow inner types to have interior mutability, which DashMap does
                    // let result = panic::catch_unwind(move || {
                    //     let _pctx = stdx::panic_context::enter(panic_context);
                    //     f(world, params)
                    // });
                    // match thread_result_to_response::<R>(req.id.clone(), result) {
                    //     Ok(response) => Task::Response(response),
                    //     Err(_) => Task::Response(lsp_server::Response::new_err(
                    //         req.id,
                    //         lsp_server::ErrorCode::ContentModified as i32,
                    //         "content modified".to_string(),
                    //     )),

                    let result = {
                        let _pctx = stdx::panic_context::enter(panic_context);
                        f(world, params)
                    };
                    match result_to_response::<R>(req.id.clone(), result) {
                        Ok(response) => Task::Response(response),
                        Err(_) => Task::Response(lsp_server::Response::new_err(
                            req.id,
                            lsp_server::ErrorCode::ContentModified as i32,
                            "content modified".to_string(),
                        )),
                    }
                }
            });

        self
    }

    /// Dispatches a non-latency-sensitive request onto the thread pool.
    pub(crate) fn on<R>(
        &mut self,
        f: fn(ServerStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request + 'static,
        R::Params: DeserializeOwned + panic::UnwindSafe + Send + fmt::Debug,
        R::Result: Serialize,
    {
        self.on_with_thread_intent::<R>(ThreadIntent::Worker, f)
    }

    /// Dispatches a latency-sensitive request onto the thread pool.
    pub(crate) fn on_latency_sensitive<R>(
        &mut self,
        f: fn(ServerStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request + 'static,
        R::Params: DeserializeOwned + panic::UnwindSafe + Send + fmt::Debug,
        R::Result: Serialize,
    {
        self.on_with_thread_intent::<R>(ThreadIntent::LatencySensitive, f)
    }

    pub(crate) fn finish(&mut self) {
        if let Some(req) = self.req.take() {
            tracing::error!("unknown request: {:?}", req);
            let response = lsp_server::Response::new_err(
                req.id,
                lsp_server::ErrorCode::MethodNotFound as i32,
                "unknown request".to_string(),
            );
            self.server_state.respond(response);
        }
    }

    fn on_with_thread_intent<R>(
        &mut self,
        intent: ThreadIntent,
        f: fn(ServerStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: lsp_types::request::Request + 'static,
        R::Params: DeserializeOwned + panic::UnwindSafe + Send + fmt::Debug,
        R::Result: Serialize,
    {
        let (req, params, panic_context) = match self.parse::<R>() {
            Some(it) => it,
            None => return self,
        };

        self.server_state
            .event_loop_state
            .task_pool
            .handle
            .spawn(intent, {
                let world = self.server_state.snapshot();
                move || {
                    // Note, RA is doing this correctly, we just cant atm because the catch_unwind doesn't
                    // allow inner types to have interior mutability, which DashMap does
                    // let result = panic::catch_unwind(move || {
                    //     let _pctx = stdx::panic_context::enter(panic_context);
                    //     f(world, params)
                    // });
                    // match thread_result_to_response::<R>(req.id.clone(), result) {
                    //     Ok(response) => Task::Response(response),
                    //     Err(_) => Task::Retry(req),
                    // }
                    let result = {
                        let _pctx = stdx::panic_context::enter(panic_context);
                        f(world, params)
                    };
                    match result_to_response::<R>(req.id.clone(), result) {
                        Ok(response) => Task::Response(response),
                        Err(_) => Task::Retry(req),
                    }
                }
            });

        self
    }

    fn parse<R>(&mut self) -> Option<(lsp_server::Request, R::Params, String)>
    where
        R: lsp_types::request::Request,
        R::Params: DeserializeOwned + fmt::Debug,
    {
        let req = match &self.req {
            Some(req) if req.method == R::METHOD => self.req.take()?,
            _ => return None,
        };

        let res = event_loop::from_json(R::METHOD, &req.params);
        match res {
            Ok(params) => {
                let panic_context = format!("\nrequest: {} {params:#?}", R::METHOD);
                Some((req, params, panic_context))
            }
            Err(err) => {
                let response = lsp_server::Response::new_err(
                    req.id,
                    lsp_server::ErrorCode::InvalidParams as i32,
                    err.to_string(),
                );
                self.server_state.respond(response);
                None
            }
        }
    }
}

fn thread_result_to_response<R>(
    id: lsp_server::RequestId,
    result: thread::Result<anyhow::Result<R::Result>>,
) -> Result<lsp_server::Response, Cancelled>
where
    R: lsp_types::request::Request,
    R::Params: DeserializeOwned,
    R::Result: Serialize,
{
    match result {
        Ok(result) => result_to_response::<R>(id, result),
        Err(panic) => {
            let panic_message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied());

            let mut message = "request handler panicked".to_string();
            if let Some(panic_message) = panic_message {
                message.push_str(": ");
                message.push_str(panic_message)
            };

            Ok(lsp_server::Response::new_err(
                id,
                lsp_server::ErrorCode::InternalError as i32,
                message,
            ))
        }
    }
}

fn result_to_response<R>(
    id: lsp_server::RequestId,
    result: anyhow::Result<R::Result>,
) -> Result<lsp_server::Response, Cancelled>
where
    R: lsp_types::request::Request,
    R::Params: DeserializeOwned,
    R::Result: Serialize,
{
    let res = match result {
        Ok(resp) => lsp_server::Response::new_ok(id, &resp),
        Err(e) => match e.downcast::<LspError>() {
            Ok(lsp_error) => lsp_server::Response::new_err(id, lsp_error.code, lsp_error.message),
            Err(e) => match e.downcast::<Cancelled>() {
                Ok(cancelled) => return Err(cancelled),
                Err(e) => lsp_server::Response::new_err(
                    id,
                    lsp_server::ErrorCode::InternalError as i32,
                    e.to_string(),
                ),
            },
        },
    };
    Ok(res)
}

pub(crate) struct NotificationDispatcher<'a> {
    pub(crate) not: Option<lsp_server::Notification>,
    pub(crate) server_state: &'a mut ServerStateExt,
}

impl<'a> NotificationDispatcher<'a> {
    pub(crate) fn on_sync_mut<N>(
        &mut self,
        f: fn(&mut ServerStateExt, N::Params) -> Result<(), LanguageServerError>,
    ) -> anyhow::Result<&mut Self>
    where
        N: lsp_types::notification::Notification,
        N::Params: DeserializeOwned + Send,
    {
        let not = match self.not.take() {
            Some(it) => it,
            None => return Ok(self),
        };
        let params = match not.extract::<N::Params>(N::METHOD) {
            Ok(it) => it,
            Err(ExtractError::JsonError { method, error }) => {
                panic!("Invalid request\nMethod: {method}\n error: {error}",)
            }
            Err(ExtractError::MethodMismatch(not)) => {
                self.not = Some(not);
                return Ok(self);
            }
        };
        let _pctx = stdx::panic_context::enter(format!("\nnotification: {}", N::METHOD));
        f(self.server_state, params)?;
        Ok(self)
    }

    pub(crate) fn finish(&mut self) {
        if let Some(not) = &self.not {
            if !not.method.starts_with("$/") {
                tracing::error!("unhandled notification: {:?}", not);
            }
        }
    }

    //experiemental 
    pub(crate) fn on_did_change<N>(
        &mut self,
        f: fn(&mut ServerStateExt, N::Params) -> Result<(), LanguageServerError>,
    ) -> anyhow::Result<&mut Self>
    where
        N: lsp_types::notification::Notification<Params = lsp_types::DidChangeTextDocumentParams>,
        N::Params: DeserializeOwned + Send,
    {
        let not = match self.not.take() {
            Some(it) => it,
            None => return Ok(self),
        };
        let params = match not.extract::<N::Params>(N::METHOD) {
            Ok(it) => it,
            Err(ExtractError::JsonError { method, error }) => {
                panic!("Invalid request\nMethod: {method}\n error: {error}",)
            }
            Err(ExtractError::MethodMismatch(not)) => {
                self.not = Some(not);
                return Ok(self);
            }
        };

        tracing::info!("did_change begin before thread");

        self.server_state
        .event_loop_state
        .task_pool
        .handle
        .spawn(ThreadIntent::Worker, {
            let state = self.server_state.snapshot();
            move || {
                let (uri, session) = state.sessions
                    .uri_and_session_from_workspace(&params.text_document.uri).unwrap();
                session.write_changes_to_file(&uri, params.content_changes).unwrap();
                if session.parse_project(&uri).unwrap() {
                    eprintln!("project parsed!!!!");
                }
                //f(world, params)

                // dummy task for now
                Task::Response(lsp_server::Response::new_err(
                    1.into(),
                    lsp_server::ErrorCode::ContentModified as i32,
                    "content modified".to_string(),
                ))
            }
        }); 
        tracing::info!("did_change thread spawned");
        Ok(self)
    }
}
