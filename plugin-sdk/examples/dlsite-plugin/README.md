# DLSite Metadata Plugin

A plugin for Archust that automatically extracts DLSite product codes from archive names and enriches them with metadata.

## Features

- Extracts DLSite product codes (RJ, VJ, BJ) from archive filenames
- Supports 6-8 digit product codes
- Cleans up download site tags and noise from filenames
- Extracts game titles from filenames
- Enriches archives with metadata (API integration pending)

## Supported Code Formats

The plugin recognizes DLSite product codes in these formats:

- `RJ123456` - Regular products (6 digits)
- `VJ01234567` - Voice/Audio products (8 digits)
- `BJ12345678` - Books/Manga (8 digits)

## Example Filenames

These filenames will be successfully processed:

- `RJ123456 Game Name.zip`
- `[DLsite] RJ123456 Game Name.7z`
- `[some-site] RJ01234567_game.rar`
- `VJ12345678.zip`

## Building

```bash
# Build for WASM
cargo build --target wasm32-unknown-unknown --release

# The output will be at:
# target/wasm32-unknown-unknown/release/dlsite_plugin.wasm
```

## Installation

1. Build the plugin
2. Copy the `.wasm` file and `dlsite-metadata.toml` to the plugins directory:
   ```bash
   cp target/wasm32-unknown-unknown/release/dlsite_plugin.wasm ~/.local/share/archust/plugins/dlsite-metadata.wasm
   cp dlsite-metadata.toml ~/.local/share/archust/plugins/
   ```

## Configuration

The plugin manifest (`dlsite-metadata.toml`) configures the plugin's capabilities and behavior:

```toml
[capabilities]
network = true                    # Required for API calls
archive_metadata_write = true     # Required to add metadata
archive_modify = true             # Required to modify archives

[rate_limits]
http_requests_per_minute = 10    # Respectful API usage
```

## How It Works

1. **Archive Opens**: When an archive is opened, the plugin receives an `OnArchiveOpen` event
2. **Code Extraction**: The plugin analyzes the filename and extracts the DLSite code
3. **Title Extraction**: Extracts the game title if present in filename
4. **Metadata Fetch**: (Pending) Queries the DLSite API for product information
5. **Archive Enrichment**: (Pending) Adds `metadata.json` to the archive

## Code Examples

### Extract DLSite Code

```rust
let code = extract_dlsite_code("[DLsite] RJ123456 Game.zip");
assert_eq!(code.full_code(), "RJ123456");
```

### Clean Filename

```rust
let cleaned = clean_filename("[DLsite] RJ123456 Game.zip");
// Returns: "RJ123456 Game.zip"
```

### Extract Title

```rust
let code = DLSiteCode { prefix: "RJ", number: "123456" };
let title = extract_title_from_filename("RJ123456 My Game.zip", &code);
// Returns: Some("My Game")
```

## Testing

Run the tests:

```bash
cargo test
```

Tests cover:
- Code extraction for RJ/VJ/BJ prefixes
- Filename cleaning
- Title extraction
- Edge cases and malformed inputs

## Future Enhancements

- [ ] DLSite API integration
- [ ] Metadata caching
- [ ] Rate limiting implementation
- [ ] Retry logic for failed requests
- [ ] Support for additional metadata fields
- [ ] Cover image download
- [ ] Multi-language support

## License

Same as Archust main project.