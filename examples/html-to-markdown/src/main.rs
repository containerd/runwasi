use std::env;
use std::fs;
use std::path::Path;

fn main() {
    // Get command line arguments
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 3 {
        eprintln!("Usage: {} <input.html> <output.md>", args[0]);
        std::process::exit(1);
    }
    
    let input_path = &args[1];
    let output_path = &args[2];
    
    // Check if input file exists
    if !Path::new(input_path).exists() {
        eprintln!("Error: Input file '{}' does not exist", input_path);
        std::process::exit(1);
    }
    
    // Read HTML content from input file
    let html_content = match fs::read_to_string(input_path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Error reading input file '{}': {}", input_path, e);
            std::process::exit(1);
        }
    };
    
    // Convert HTML to Markdown
    let markdown_content = html2md::parse_html(&html_content);
    
    // Write markdown content to output file
    if let Err(e) = fs::write(output_path, markdown_content) {
        eprintln!("Error writing output file '{}': {}", output_path, e);
        std::process::exit(1);
    }
    
    println!("Successfully converted '{}' to '{}'", input_path, output_path);
}
