#!/usr/bin/env nu

# Ize Development Watch Script for Nu Shell
# Automatically runs tests when test files change, builds when source files change

# Check if cargo-watch is installed
def check-cargo-watch [] {
    if (which cargo-watch | is-empty) {
        print $"(ansi yellow)cargo-watch is not installed.(ansi reset)"
        print "Installing cargo-watch..."
        cargo install cargo-watch
    }
}

# Main watch function
def main [] {
    print $"(ansi blue)Starting Ize watch mode...(ansi reset)"
    print $"(ansi green)• Test files \(tests/**/*.rs\) → cargo test(ansi reset)"
    print $"(ansi green)• Source files \(src/**/*.rs\) → cargo build(ansi reset)"
    print ""

    # Check for cargo-watch
    check-cargo-watch

    # Get script directory and project root
    let script_dir = ($env.FILE_PWD? | default (pwd))
    let project_root = ($script_dir | path dirname)

    # Change to the library directory
    cd ($project_root | path join "crates" "Ize-lib")

    # Use Nu's built-in watch command for simple watching
    # For more complex logic, we'll use cargo-watch with a custom command
    cargo watch --clear --why --watch src --watch tests --watch Cargo.toml --shell $'nu -c "
        # Get the changed files from environment variable
        let reason = \($env.CARGO_WATCH_REASON? | default \"\")

        if \($reason | str contains \"tests/\") {
            print \(ansi blue)\"Test files changed, running tests...\(ansi reset)\"
            cargo test --color=always
        } else if \($reason | str contains \"Cargo.toml\") {
            print \(ansi blue)\"Cargo.toml changed, running build and tests...\(ansi reset)\"
            cargo build --color=always
            cargo test --color=always
        } else {
            print \(ansi blue)\"Source files changed, building...\(ansi reset)\"
            cargo build --color=always
        }
    "'
}

# Alternative implementation using Nu's watch command directly
def "main alt" [] {
    print $"(ansi blue)Starting Ize watch mode with Nu's built-in watch...(ansi reset)"
    print $"(ansi green)Watching for changes in src/ and tests/(ansi reset)"
    print ""

    # Get script directory and project root
    let script_dir = ($env.FILE_PWD? | default (pwd))
    let project_root = ($script_dir | path dirname)

    cd ($project_root | path join "crates" "Ize-lib")

    # Nu's watch command with custom handler
    watch . --glob=**/*.rs --glob=**/Cargo.toml {|op, path|
        let changed_path = ($path | str join)
        print $"File changed: ($changed_path)"

        if ($changed_path | str contains "tests/") {
            print $"(ansi blue)Test file changed, running tests...(ansi reset)"
            cargo test --color=always
        } else if ($changed_path | str ends-with "Cargo.toml") {
            print $"(ansi blue)Cargo.toml changed, running build and tests...(ansi reset)"
            cargo build --color=always
            cargo test --color=always
        } else {
            print $"(ansi blue)Source file changed, building...(ansi reset)"
            cargo build --color=always
        }
    }
}

# Run tests only
def "main test" [] {
    print $"(ansi blue)Starting test watch mode...(ansi reset)"
    # Get script directory and project root
    let script_dir = ($env.FILE_PWD? | default (pwd))
    let project_root = ($script_dir | path dirname)

    cd ($project_root | path join "crates" "Ize-lib")
    cargo watch --clear -x test
}

# Run build only
def "main build" [] {
    print $"(ansi blue)Starting build watch mode...(ansi reset)"
    # Get script directory and project root
    let script_dir = ($env.FILE_PWD? | default (pwd))
    let project_root = ($script_dir | path dirname)

    cd ($project_root | path join "crates" "Ize-lib")
    cargo watch --clear -x build
}

# Run with specific features
def "main with-features" [features: string] {
    print $"(ansi blue)Starting watch mode with features: ($features)(ansi reset)"
    # Get script directory and project root
    let script_dir = ($env.FILE_PWD? | default (pwd))
    let project_root = ($script_dir | path dirname)

    cd ($project_root | path join "crates" "Ize-lib")
    cargo watch --clear -x $"build --features ($features)" -x $"test --features ($features)"
}

# Show help
def "main help" [] {
    print "Ize Watch Script Commands:"
    print ""
    print "  watch.nu              - Watch and run tests/build based on changed files"
    print "  watch.nu alt          - Use Nu's built-in watch (experimental)"
    print "  watch.nu test         - Watch and run tests only"
    print "  watch.nu build        - Watch and build only"
    print "  watch.nu with-features <features> - Watch with specific Cargo features"
    print "  watch.nu help         - Show this help message"
}
