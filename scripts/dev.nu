#!/usr/bin/env nu

# Claris-FUSE Development Tools for Nu Shell
# A comprehensive development script with watch functionality

# Custom type for watch events
def watch-event-type [] {
    ["test", "build", "all", "bench", "clippy", "fmt"]
}

# Check if required tools are installed
def check-tools [] {
    let tools = [
        {name: "cargo-watch", cmd: "cargo-watch", install: "cargo install cargo-watch"}
        {name: "cargo-nextest", cmd: "cargo-nextest", install: "cargo install cargo-nextest"}
    ]

    let missing = $tools | where {|tool|
        (which $tool.cmd | is-empty)
    }

    if ($missing | length) > 0 {
        print $"(ansi yellow)Missing development tools:(ansi reset)"
        $missing | each {|tool|
            print $"  • ($tool.name): ($tool.install)"
        }
        print ""

        if (input "Install missing tools? [y/N] " | str downcase) == "y" {
            $missing | each {|tool|
                print $"Installing ($tool.name)..."
                nu -c $tool.install
            }
        }
    }
}

# Get project root directory
def get-project-root [] {
    # First try to use the script's directory if available
    let start_dir = if ($env.FILE_PWD? | is-not-empty) {
        # If we're in the scripts directory, go up one level
        let script_dir = $env.FILE_PWD
        if ($script_dir | path basename) == "scripts" {
            $script_dir | path dirname
        } else {
            $script_dir
        }
    } else {
        pwd
    }

    # Look for project markers
    let markers = ["Cargo.toml", ".git"]
    mut dir = $start_dir

    while $dir != "/" {
        for marker in $markers {
            let marker_path = ($dir | path join $marker)
            if ($marker_path | path exists) {
                # If we found Cargo.toml, check if it's the workspace root
                if $marker == "Cargo.toml" {
                    let content = open $marker_path
                    if ($content | str contains "[workspace]") {
                        return $dir
                    }
                } else {
                    return $dir
                }
            }
        }
        $dir = ($dir | path dirname)
    }

    # Fallback to start directory
    return $start_dir
}

# Watch for file changes and run appropriate commands
def "dev watch" [
    --test-only(-t)  # Only run tests
    --build-only(-b) # Only run builds
    --all(-a)        # Run all checks
    --clear(-c)      # Clear screen before each run
] {
    check-tools

    let root = get-project-root
    cd $"($root)/crates/claris-fuse-lib"

    print $"(ansi blue)◆ Starting Claris-FUSE watch mode(ansi reset)"
    print $"  (ansi dim)Project root: ($root)(ansi reset)"

    if $test_only {
        print $"  (ansi green)Mode: Test only(ansi reset)"
        cargo watch --clear -x test
    } else if $build_only {
        print $"  (ansi green)Mode: Build only(ansi reset)"
        cargo watch --clear -x build
    } else if $all {
        print $"  (ansi green)Mode: All checks (fmt, clippy, test, build)(ansi reset)"
        cargo watch --clear -x fmt -x clippy -x test -x build
    } else {
        # Smart mode: different commands based on what changed
        print $"  (ansi green)Mode: Smart \(tests for test files, build for source\)(ansi reset)"
        print ""

        # Use cargo-watch with a Nu script as the shell command
        cargo watch --clear --watch src --watch tests --watch Cargo.toml --shell 'nu -c "
            let changed = ($env.CARGO_WATCH_REASON? | default \"\")

            # Determine what type of file changed
            let change_type = if ($changed | str contains \"tests/\") {
                \"test\"
            } else if ($changed | str contains \"Cargo.toml\") {
                \"config\"
            } else {
                \"source\"
            }

            # Print what changed
            print \"\"
            print $\"(ansi yellow)◆ Change detected:(ansi reset) ($changed)\"

            # Run appropriate commands
            match $change_type {
                \"test\" => {
                    print $\"(ansi blue)→ Running tests...(ansi reset)\"
                    cargo test --color=always
                }
                \"config\" => {
                    print $\"(ansi blue)→ Configuration changed, rebuilding and testing...(ansi reset)\"
                    cargo build --color=always
                    if $? { cargo test --color=always }
                }
                \"source\" => {
                    print $\"(ansi blue)→ Building...(ansi reset)\"
                    cargo build --color=always
                }
            }
        "'
    }
}

# Run specific test(s)
def "dev test" [
    filter?: string  # Optional test filter
    --watch(-w)      # Watch mode
    --nextest(-n)    # Use cargo-nextest
] {
    cd $"(get-project-root)/crates/claris-fuse-lib"

    let cmd = if $nextest { "cargo nextest run" } else { "cargo test" }
    let full_cmd = if ($filter | is-empty) { $cmd } else { $"($cmd) ($filter)" }

    if $watch {
        cargo watch --clear -x $"($cmd) ($filter)"
    } else {
        nu -c $full_cmd
    }
}

# Build the project
def "dev build" [
    --release(-r)    # Release mode
    --watch(-w)      # Watch mode
    --features(-f): string  # Features to enable
] {
    cd $"(get-project-root)/crates/claris-fuse-lib"

    mut cmd_parts = ["cargo", "build"]
    if $release { $cmd_parts = ($cmd_parts | append "--release") }
    if not ($features | is-empty) {
        $cmd_parts = ($cmd_parts | append ["--features", $features])
    }

    let cmd = ($cmd_parts | str join " ")

    if $watch {
        cargo watch --clear -x ($cmd_parts | skip 1 | str join " ")
    } else {
        nu -c $cmd
    }
}

# Run clippy
def "dev clippy" [
    --watch(-w)      # Watch mode
    --fix            # Auto-fix issues
] {
    cd $"(get-project-root)/crates/claris-fuse-lib"

    let cmd = if $fix {
        "cargo clippy --fix --allow-dirty --allow-staged"
    } else {
        "cargo clippy -- -D warnings"
    }

    if $watch {
        cargo watch --clear -s $cmd
    } else {
        nu -c $cmd
    }
}

# Format code
def "dev fmt" [
    --check(-c)      # Check only, don't modify
    --watch(-w)      # Watch mode
] {
    cd get-project-root

    let cmd = if $check { "cargo fmt --all -- --check" } else { "cargo fmt --all" }

    if $watch {
        cargo watch --clear -s $cmd
    } else {
        nu -c $cmd
    }
}

# Run all checks
def "dev check" [] {
    print $"(ansi blue)◆ Running all checks...(ansi reset)\n"

    let checks = [
        {name: "Format", cmd: "dev fmt --check"}
        {name: "Clippy", cmd: "dev clippy"}
        {name: "Build", cmd: "dev build"}
        {name: "Tests", cmd: "dev test"}
    ]

    let results = $checks | each {|check|
        print $"(ansi yellow)→ Running ($check.name)...(ansi reset)"
        let start = (date now)

        let result = (do { nu -c $check.cmd; true } | complete)
        let duration = ((date now) - $start)

        if $result.exit_code == 0 {
            print $"  (ansi green)✓ ($check.name) passed(ansi reset) (ansi dim)\(($duration | format duration)\)(ansi reset)\n"
            {name: $check.name, status: "✓", duration: ($duration | format duration)}
        } else {
            print $"  (ansi red)✗ ($check.name) failed(ansi reset)\n"
            {name: $check.name, status: "✗", duration: ($duration | format duration)}
        }
    }

    print "\nSummary:"
    print ($results | table -n 0)

    let failed = ($results | where status == "✗" | length)
    if $failed > 0 {
        print $"\n(ansi red)($failed) check\(s\) failed(ansi reset)"
        exit 1
    } else {
        print $"\n(ansi green)All checks passed!(ansi reset)"
    }
}

# Clean build artifacts
def "dev clean" [
    --deep(-d)       # Deep clean (remove target directory)
] {
    cd get-project-root

    if $deep {
        print "Performing deep clean..."
        rm -rf target
        rm -rf Cargo.lock
    } else {
        cargo clean
    }

    print $"(ansi green)✓ Clean complete(ansi reset)"
}

# Show project statistics
def "dev stats" [] {
    cd get-project-root

    print $"(ansi blue)◆ Claris-FUSE Project Statistics(ansi reset)\n"

    # Count lines of code
    let rust_files = (ls **/*.rs | where type == "file")
    let total_lines = ($rust_files | each {|f| open $f.name | lines | length} | math sum)
    let file_count = ($rust_files | length)

    # Get test count
    let test_count = (
        $rust_files
        | each {|f| open $f.name | lines | where {|line| $line | str contains "#[test]"} | length}
        | math sum
    )

    # Create stats table
    let stats = [
        ["Metric", "Value"];
        ["Rust files", $file_count]
        ["Total lines", $total_lines]
        ["Test functions", $test_count]
        ["Avg lines/file", ($total_lines / $file_count | math round)]
    ]

    print ($stats | table)

    # Show file type breakdown
    print $"\n(ansi yellow)File breakdown:(ansi reset)"
    ls **/*.*
    | where type == "file"
    | get name
    | path parse
    | get extension
    | uniq -c
    | sort-by count --reverse
    | first 10
    | table
}

# Main help
def "dev" [] {
    print $"(ansi blue)◆ Claris-FUSE Development Tools(ansi reset)\n"
    print "Available commands:\n"

    let commands = [
        ["Command", "Description"];
        ["dev watch", "Watch files and run tests/builds automatically"]
        ["dev test", "Run tests (with optional filter)"]
        ["dev build", "Build the project"]
        ["dev clippy", "Run clippy lints"]
        ["dev fmt", "Format code"]
        ["dev check", "Run all checks (fmt, clippy, build, test)"]
        ["dev clean", "Clean build artifacts"]
        ["dev stats", "Show project statistics"]
    ]

    print ($commands | table)
    print $"\nUse (ansi yellow)dev <command> --help(ansi reset) for more options"
}

# Export the main command
alias dev = dev
