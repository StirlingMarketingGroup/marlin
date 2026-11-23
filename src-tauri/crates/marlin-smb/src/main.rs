//! SMB helper binary. Isolated process for SMB operations.
//!
//! This binary is a JSON-RPC 2.0 server over stdin/stdout.
//! It handles all SMB operations, keeping libsmbclient out of the main app.
//!
//! The sidecar:
//! - Reads JSON-RPC requests from stdin (one per line, NDJSON format)
//! - Executes SMB operations using pavao/libsmbclient
//! - Writes JSON-RPC responses to stdout (one per line)
//! - Logs to stderr only (stdout is reserved for IPC)
//! - Exits when stdin closes (prevents orphan processes)

mod operations;

use marlin_smb_client::protocol::{error_codes, methods, PingResult, Request, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{BufRead, Write};

/// Run the sidecar main loop.
/// This function reads JSON-RPC requests from stdin and writes responses to stdout.
/// It exits when stdin is closed (EOF).
fn main() {
    eprintln!(
        "[marlin-smb] Starting sidecar v{}",
        operations::SIDECAR_VERSION
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdout_lock = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[marlin-smb] Error reading stdin: {}", e);
                break;
            }
        };

        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        // Parse the request
        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[marlin-smb] Failed to parse request: {}", e);
                // Write a parse error response (use id 0 since we can't get the real id)
                let response =
                    Response::error(0, error_codes::PARSE_ERROR, format!("Parse error: {}", e));
                write_response(&mut stdout_lock, &response);
                continue;
            }
        };

        // Dispatch to the appropriate handler
        let response = dispatch_request(&request);

        // Write the response
        write_response(&mut stdout_lock, &response);
    }

    eprintln!("[marlin-smb] Stdin closed, exiting");
}

/// Write a response to stdout (with newline).
fn write_response<W: Write>(writer: &mut W, response: &Response) {
    match serde_json::to_string(response) {
        Ok(json) => {
            if let Err(e) = writeln!(writer, "{}", json) {
                eprintln!("[marlin-smb] Failed to write response: {}", e);
            }
            if let Err(e) = writer.flush() {
                eprintln!("[marlin-smb] Failed to flush stdout: {}", e);
            }
        }
        Err(e) => {
            eprintln!("[marlin-smb] Failed to serialize response: {}", e);
        }
    }
}

/// Dispatch a request to the appropriate handler.
fn dispatch_request(request: &Request) -> Response {
    match request.method.as_str() {
        methods::PING => handle_ping(request),
        methods::READ_DIRECTORY => respond(request, operations::read_directory),
        methods::GET_FILE_METADATA => respond(request, operations::get_file_metadata),
        methods::CREATE_DIRECTORY => respond(request, operations::create_directory),
        methods::DELETE => respond(request, operations::delete),
        methods::RENAME => respond(request, operations::rename),
        methods::COPY => respond(request, operations::copy),
        methods::LIST_SHARES => respond(request, operations::list_shares),
        methods::READ => respond(request, operations::read),
        methods::SPACE => respond(request, operations::space),
        methods::DOWNLOAD_FILE => respond(request, operations::download_file),
        methods::DOWNLOAD_PARTIAL => respond(request, operations::download_partial),
        methods::UPLOAD_FILE => respond(request, operations::upload_file),
        _ => Response::error(
            request.id,
            error_codes::METHOD_NOT_FOUND,
            format!("Method not found: {}", request.method),
        ),
    }
}

fn handle_ping(request: &Request) -> Response {
    Response::success(
        request.id,
        PingResult {
            version: operations::SIDECAR_VERSION.to_string(),
        },
    )
}

/// Reads the params, runs `operation`, and answers with its result or its error.
fn respond<P: DeserializeOwned, R: Serialize>(
    request: &Request,
    operation: fn(P) -> Result<R, (i32, String)>,
) -> Response {
    let params: P = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operation(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}
