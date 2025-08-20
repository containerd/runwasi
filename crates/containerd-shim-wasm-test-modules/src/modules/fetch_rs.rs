/// Fetch example with WebSearch functionality
/// 
/// This example demonstrates HTTP client functionality and includes a WebSearch function
/// that can query search APIs with configurable parameters.

/// WebSearch parameters structure
#[derive(Debug)]
pub struct WebSearchParams {
    /// The search query to look up
    pub query: String,
    /// Number of results to return
    pub num_results: u32,
    /// Language code (optional, e.g., 'en')
    pub language: Option<String>,
    /// Region code (optional, e.g., 'us')
    pub region: Option<String>,
}

impl WebSearchParams {
    pub fn new(query: String, num_results: u32) -> Self {
        Self {
            query,
            num_results,
            language: None,
            region: None,
        }
    }

    pub fn with_language(mut self, language: String) -> Self {
        self.language = Some(language);
        self
    }

    pub fn with_region(mut self, region: String) -> Self {
        self.region = Some(region);
        self
    }
}

/// WebSearch result item
#[derive(Debug)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Perform a web search using the provided parameters
pub fn web_search(params: &WebSearchParams) -> Result<Vec<WebSearchResult>, String> {
    println!("Performing web search with parameters:");
    println!("  Query: {}", params.query);
    println!("  Number of results: {}", params.num_results);
    
    if let Some(ref lang) = params.language {
        println!("  Language: {}", lang);
    }
    
    if let Some(ref region) = params.region {
        println!("  Region: {}", region);
    }

    // Build search URL
    let search_url = build_search_url(params)?;
    println!("  Search URL: {}", search_url);

    // For demonstration purposes, we'll create mock results
    // In a real implementation, this would make an HTTP request to a search API
    let mut results = Vec::new();
    
    for i in 1..=(params.num_results.min(5)) {
        results.push(WebSearchResult {
            title: format!("Search Result {} for '{}'", i, params.query),
            url: format!("https://example.com/result/{}", i),
            snippet: format!(
                "This is a snippet for search result {} showing relevant content about '{}'...", 
                i, params.query
            ),
        });
    }

    Ok(results)
}

/// Build the search URL with parameters
fn build_search_url(params: &WebSearchParams) -> Result<String, String> {
    // Using a mock search API endpoint for demonstration
    // In production, this would be a real search API like Google Custom Search API
    let mut url = format!(
        "https://api.mockservice.com/search?q={}&count={}",
        url_encode(&params.query),
        params.num_results
    );
    
    if let Some(ref lang) = params.language {
        url.push_str(&format!("&hl={}", url_encode(lang)));
    }
    
    if let Some(ref region) = params.region {
        url.push_str(&format!("&gl={}", url_encode(region)));
    }
    
    Ok(url)
}

/// Simple URL encoding for query parameters
fn url_encode(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            c if c.is_alphanumeric() => c.to_string(),
            c => format!("%{:02X}", c as u8),
        })
        .collect()
}

/// Demonstrate fetch functionality with WebSearch
fn demonstrate_fetch_with_websearch() {
    println!("=== Fetch Example with WebSearch Function ===");
    println!();

    // Example 1: Basic web search
    println!("Example 1: Basic web search");
    let params1 = WebSearchParams::new("rust programming".to_string(), 3);
    
    match web_search(&params1) {
        Ok(results) => {
            println!("Found {} results:", results.len());
            for (index, result) in results.iter().enumerate() {
                println!("  {}. {}", index + 1, result.title);
                println!("     URL: {}", result.url);
                println!("     Snippet: {}", result.snippet);
                println!();
            }
        }
        Err(e) => {
            eprintln!("Search failed: {}", e);
        }
    }

    println!("----------------------------------------");
    println!();

    // Example 2: Web search with language and region
    println!("Example 2: Web search with language and region");
    let params2 = WebSearchParams::new("webassembly tutorial".to_string(), 5)
        .with_language("en".to_string())
        .with_region("us".to_string());
    
    match web_search(&params2) {
        Ok(results) => {
            println!("Found {} results:", results.len());
            for (index, result) in results.iter().enumerate() {
                println!("  {}. {}", index + 1, result.title);
                println!("     URL: {}", result.url);
                println!("     Snippet: {}", result.snippet);
                println!();
            }
        }
        Err(e) => {
            eprintln!("Search failed: {}", e);
        }
    }
}

fn main() {
    demonstrate_fetch_with_websearch();
}