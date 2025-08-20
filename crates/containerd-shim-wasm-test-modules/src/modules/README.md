# Fetch Example with WebSearch Function

This directory contains the source code for a WebAssembly module that demonstrates HTTP client functionality (fetch) with an integrated WebSearch function.

## Features

The `fetch_rs.rs` module provides:

1. **WebSearch Function**: A comprehensive search function with the following parameters:
   - `query` (required): The search query to look up
   - `num_results` (required): Number of results to return
   - `language` (optional): Language code (e.g., 'en', 'fr', 'de')
   - `region` (optional): Region code (e.g., 'us', 'uk', 'ca')

2. **WebSearchParams Structure**: A builder pattern for configuring search parameters
3. **WebSearchResult Structure**: Represents individual search results with title, URL, and snippet
4. **URL Encoding**: Proper encoding of search parameters for HTTP requests
5. **Error Handling**: Comprehensive error handling for various failure scenarios

## API Usage

### Basic Search
```rust
let params = WebSearchParams::new("rust programming".to_string(), 3);
let results = web_search(&params)?;
```

### Search with Language and Region
```rust
let params = WebSearchParams::new("webassembly tutorial".to_string(), 5)
    .with_language("en".to_string())
    .with_region("us".to_string());
let results = web_search(&params)?;
```

## Example Output

When the module is executed, it demonstrates both basic and advanced usage:

```
=== Fetch Example with WebSearch Function ===

Example 1: Basic web search
Performing web search with parameters:
  Query: rust programming
  Number of results: 3
  Search URL: https://api.mockservice.com/search?q=rust%20programming&count=3
Found 3 results:
  1. Search Result 1 for 'rust programming'
     URL: https://example.com/result/1
     Snippet: This is a snippet for search result 1 showing relevant content about 'rust programming'...

  2. Search Result 2 for 'rust programming'
     URL: https://example.com/result/2
     Snippet: This is a snippet for search result 2 showing relevant content about 'rust programming'...

  3. Search Result 3 for 'rust programming'
     URL: https://example.com/result/3
     Snippet: This is a snippet for search result 3 showing relevant content about 'rust programming'...

----------------------------------------

Example 2: Web search with language and region
Performing web search with parameters:
  Query: webassembly tutorial
  Number of results: 5
  Language: en
  Region: us
  Search URL: https://api.mockservice.com/search?q=webassembly%20tutorial&count=5&hl=en&gl=us
Found 5 results:
  1. Search Result 1 for 'webassembly tutorial'
     URL: https://example.com/result/1
     Snippet: This is a snippet for search result 1 showing relevant content about 'webassembly tutorial'...
  
  [... additional results ...]
```

## Implementation Notes

- The current implementation uses mock search results for demonstration purposes
- In a production environment, this would integrate with real search APIs (Google Custom Search, Bing Search API, etc.)
- The module follows WASI patterns and can be run in WebAssembly runtimes that support WASI
- URL encoding is implemented to ensure proper handling of special characters in search queries
- The builder pattern for `WebSearchParams` makes the API easy to use and extend

## Building

The module can be built using rustc with the wasm32-wasip1 target:

```bash
rustc --target=wasm32-wasip1 -Copt-level=z -Cstrip=symbols -o fetch_rs.wasm fetch_rs.rs
```

## Integration

This module is designed to work with the runwasi shim infrastructure and can be deployed as a WebAssembly container using containerd with the appropriate wasmtime shim.