//! SMB sidecar - isolated process for SMB operations.
//!
//! This module implements a JSON-RPC 2.0 server over stdin/stdout.
//! It is compiled as a separate binary (`marlin-smb`) that handles
//! all SMB operations, isolating libsmbclient from the main app.
//!
//! The sidecar:
//! - Reads JSON-RPC requests from stdin (one per line, NDJSON format)
//! - Executes SMB operations using pavao/libsmbclient
//! - Writes JSON-RPC responses to stdout (one per line)
//! - Logs to stderr only (stdout is reserved for IPC)
//! - Exits when stdin closes (prevents orphan processes)

pub mod operations;
pub mod protocol;

use protocol::{
    error_codes, methods, CopyParams, CreateDirectoryParams, DeleteParams, DownloadFileParams,
    GetFileMetadataParams, ListSharesParams, PingResult, ReadDirectoryParams, RenameParams,
    Request, Response, TestConnectionParams, UploadFileParams,
};
use std::io::{BufRead, Write};

/// Run the sidecar main loop.
/// This function reads JSON-RPC requests from stdin and writes responses to stdout.
/// It exits when stdin is closed (EOF).
pub fn run() {
    eprintln!("[marlin-smb] Starting sidecar v{}", operations::SIDECAR_VERSION);

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
                let response = Response::error(0, error_codes::PARSE_ERROR, format!("Parse error: {}", e));
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
        methods::READ_DIRECTORY => handle_read_directory(request),
        methods::GET_FILE_METADATA => handle_get_file_metadata(request),
        methods::CREATE_DIRECTORY => handle_create_directory(request),
        methods::DELETE => handle_delete(request),
        methods::RENAME => handle_rename(request),
        methods::COPY => handle_copy(request),
        methods::LIST_SHARES => handle_list_shares(request),
        methods::TEST_CONNECTION => handle_test_connection(request),
        methods::DOWNLOAD_FILE => handle_download_file(request),
        methods::UPLOAD_FILE => handle_upload_file(request),
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

fn handle_read_directory(request: &Request) -> Response {
    let params: ReadDirectoryParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::read_directory(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_get_file_metadata(request: &Request) -> Response {
    let params: GetFileMetadataParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::get_file_metadata(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_create_directory(request: &Request) -> Response {
    let params: CreateDirectoryParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::create_directory(params) {
        Ok(()) => Response::success(request.id, serde_json::Value::Null),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_delete(request: &Request) -> Response {
    let params: DeleteParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::delete(params) {
        Ok(()) => Response::success(request.id, serde_json::Value::Null),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_rename(request: &Request) -> Response {
    let params: RenameParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::rename(params) {
        Ok(()) => Response::success(request.id, serde_json::Value::Null),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_copy(request: &Request) -> Response {
    let params: CopyParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::copy(params) {
        Ok(()) => Response::success(request.id, serde_json::Value::Null),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_list_shares(request: &Request) -> Response {
    let params: ListSharesParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::list_shares(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_test_connection(request: &Request) -> Response {
    let params: TestConnectionParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::test_connection(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_download_file(request: &Request) -> Response {
    let params: DownloadFileParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::download_file(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}

fn handle_upload_file(request: &Request) -> Response {
    let params: UploadFileParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return Response::error(
                request.id,
                error_codes::INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }
    };

    match operations::upload_file(params) {
        Ok(result) => Response::success(request.id, result),
        Err((code, msg)) => Response::error(request.id, code, msg),
    }
}
