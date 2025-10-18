# Configurable Logging with Tracing

The RTP Pipeline Manager now uses the `tracing` crate for structured, configurable logging. You can control what gets printed to the console using environment variables.

## Features

- **Configurable Logging**: Control verbosity with environment variables
- **Pipeline Latency Measurement**: Query actual GStreamer pipeline latency
- **Real-time Monitoring**: Track live vs non-live pipeline characteristics

## Commands

- `list` - Show pipelines with basic latency info when running
- `latency` - Show detailed latency information for all pipelines

## Log Levels Available

- **ERROR**: Critical errors and failures
- **WARN**: Warnings and non-critical issues  
- **INFO**: General information (pipeline state changes, network interface selection, etc.)
- **DEBUG**: Detailed debugging information (SSRC handling, pad connections, element properties)
- **TRACE**: Very verbose debugging (every pad event, property listing, etc.)

## Usage Examples

### Default behavior (INFO level)
```bash
cargo run
```

### Show only errors and warnings
```bash
RUST_LOG=warn cargo run
```

### Show all information including debug messages
```bash
RUST_LOG=debug cargo run
```

### Show everything including trace-level details
```bash
RUST_LOG=trace cargo run
```

### Filter by specific module
```bash
RUST_LOG=gstreamer_pipeline_manager=debug cargo run
```

### Mixed levels
```bash
RUST_LOG="info,gstreamer_pipeline_manager=debug" cargo run
```

## What Each Level Shows

### ERROR Level
- Critical pipeline failures
- Invalid command arguments
- Fatal system errors

### WARN Level  
- Pipeline already running/stopped warnings
- Network interface issues
- Non-critical operation failures

### INFO Level
- Pipeline state changes (started, stopped, restarted)
- Network interface selection
- Successful operations
- User-relevant status updates

### DEBUG Level
- SSRC switching operations
- Element property configuration
- Pad connection details
- Ghost pad management
- Pipeline cleanup operations

### TRACE Level
- Every pad addition event
- Element property enumeration
- Detailed SSRC matching logic
- Low-level GStreamer operations

## Default Configuration

If no `RUST_LOG` is set, the application defaults to `INFO` level for the main application module, showing important status updates without overwhelming debug information.

The user interface messages (command prompt, help text, etc.) are not affected by log levels and will always be shown.