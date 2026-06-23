package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// urlPattern matches the leading scheme the bash CLI uses to route an argument
// to the browser instead of the filesystem (cmux/bin/cmux `open` case).
var urlPattern = regexp.MustCompile(`^https?://`)

// runBashCompat implements the human-facing command vocabulary of the local
// bash `cmux` CLI (cmux/bin/cmux) so that commands typed in a remote shell —
// `cmux open`, `cmux notes`, `cmux tree`, `cmux list`, … — behave identically
// to the local CLI. It mirrors the bash cases byte-for-byte on the wire (V1
// text or V2 JSON) so the local socket handlers behave the same.
//
// It is consulted before the long-form command registry; for names that exist
// in both (e.g. `send`), the bash-compatible, positional form wins because that
// is what a person types interactively. Returns (exitCode, true) when it
// handled the command, or (0, false) to fall through to the registry.
func runBashCompat(socketPath, name string, args []string, jsonOutput bool, refreshAddr func() string) (int, bool) {
	switch name {
	case "tree":
		return emitV2(socketPath, "system.tree", map[string]any{}, jsonOutput, refreshAddr), true
	case "list", "ls":
		return emitV1(socketPath, "list", refreshAddr), true
	case "new":
		cmd := "new"
		if len(args) > 0 {
			cmd += " " + strings.Join(args, " ")
		}
		return emitV1(socketPath, cmd, refreshAddr), true
	case "select", "sel":
		first := ""
		if len(args) > 0 {
			first = args[0]
		}
		return emitV1(socketPath, strings.TrimRight("select "+first, " "), refreshAddr), true
	case "current":
		return emitV1(socketPath, "current", refreshAddr), true
	case "close":
		cmd := "close_workspace"
		if len(args) > 0 {
			cmd += " " + strings.Join(args, " ")
		}
		return emitV1(socketPath, cmd, refreshAddr), true
	case "send":
		if len(args) == 0 {
			fmt.Fprintln(os.Stderr, "usage: cmux send <text>")
			return 2, true
		}
		return emitV1(socketPath, "send "+strings.Join(args, " "), refreshAddr), true
	case "read", "read-screen":
		return emitV1(socketPath, "read_screen"+readScreenFlags(args), refreshAddr), true
	case "diff":
		return cmdDiff(socketPath, args, jsonOutput, refreshAddr), true
	case "project":
		return emitV2(socketPath, "workspace.new_project",
			map[string]any{"directory": firstPositionalOrCwd(args)}, jsonOutput, refreshAddr), true
	case "notes":
		return cmdNotes(socketPath, args, jsonOutput, refreshAddr), true
	case "open":
		return cmdOpen(socketPath, args, jsonOutput, refreshAddr), true
	}
	return 0, false
}

// emitV1 sends a v1 text command and prints the raw response (mirrors execV1).
func emitV1(socketPath, command string, refreshAddr func() string) int {
	resp, err := socketRoundTrip(socketPath, command, refreshAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "cmux: %v\n", err)
		return 1
	}
	fmt.Print(resp)
	if !strings.HasSuffix(resp, "\n") {
		fmt.Println()
	}
	return 0
}

// emitV2 sends a v2 JSON-RPC request and prints the result (mirrors execV2).
func emitV2(socketPath, method string, params map[string]any, jsonOutput bool, refreshAddr func() string) int {
	resp, err := socketRoundTripV2(socketPath, method, params, refreshAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "cmux: %v\n", err)
		return 1
	}
	if jsonOutput {
		fmt.Println(resp)
	} else {
		fmt.Println(defaultRelayOutput(resp))
	}
	return 0
}

// readScreenFlags rebuilds the read_screen flag tail exactly as the bash CLI
// does (cmux/bin/cmux `read|read-screen` case): --scrollback, --lines=N,
// --panel=ID (also accepting the --surface= alias).
func readScreenFlags(args []string) string {
	var b strings.Builder
	for i := 0; i < len(args); i++ {
		switch {
		case args[i] == "--scrollback":
			b.WriteString(" --scrollback")
		case args[i] == "--lines":
			if i+1 < len(args) {
				b.WriteString(" --lines=" + args[i+1])
				i++
			}
		case strings.HasPrefix(args[i], "--lines="):
			b.WriteString(" " + args[i])
		case args[i] == "--panel":
			if i+1 < len(args) {
				b.WriteString(" --panel=" + args[i+1])
				i++
			}
		case strings.HasPrefix(args[i], "--panel="), strings.HasPrefix(args[i], "--surface="):
			b.WriteString(" " + args[i])
		}
	}
	return b.String()
}

// cmdDiff mirrors the bash `diff` case: workspace.new_diff against a directory
// (default: the remote cwd) with an optional --staged / --branch <ref> source.
func cmdDiff(socketPath string, args []string, jsonOutput bool, refreshAddr func() string) int {
	source := ""
	dir := ""
	for i := 0; i < len(args); i++ {
		switch {
		case args[i] == "--staged" || args[i] == "--cached":
			source = "staged"
		case args[i] == "--unstaged":
			source = ""
		case args[i] == "--branch":
			ref := "HEAD"
			if i+1 < len(args) {
				ref = args[i+1]
				i++
			}
			source = "branch:" + ref
		case strings.HasPrefix(args[i], "--branch="):
			source = "branch:" + strings.TrimPrefix(args[i], "--branch=")
		default:
			dir = args[i]
		}
	}
	if dir == "" {
		dir = cwd()
	}
	params := map[string]any{"directory": dir}
	if source != "" {
		params["source"] = source
	}
	return emitV2(socketPath, "workspace.new_diff", params, jsonOutput, refreshAddr)
}

// cmdNotes mirrors the bash `notes` case: notes.open with a split direction
// (default "right") and an optional file resolved like `realpath -m` (absolute
// even when the file does not yet exist).
func cmdNotes(socketPath string, args []string, jsonOutput bool, refreshAddr func() string) int {
	direction := "right"
	file := ""
	for _, a := range args {
		switch a {
		case "--tab":
			direction = "tab"
		case "--down", "--vertical":
			direction = "down"
		case "--right", "--horizontal":
			direction = "right"
		default:
			file = a
		}
	}
	params := map[string]any{"direction": direction}
	if file != "" {
		params["file"] = absPath(file)
	}
	return emitV2(socketPath, "notes.open", params, jsonOutput, refreshAddr)
}

// cmdOpen mirrors the bash `open` case: URLs open a browser split, directories
// open a new workspace, and files are collected into a single file.open call.
// Paths are resolved on the remote host, so the local UI receives absolute
// remote paths.
func cmdOpen(socketPath string, args []string, jsonOutput bool, refreshAddr func() string) int {
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "usage: cmux open <path-or-url>...")
		return 2
	}
	rc := 0
	var files []any
	for _, t := range args {
		switch {
		case urlPattern.MatchString(t):
			if code := emitV2(socketPath, "browser.open_split",
				map[string]any{"url": t}, jsonOutput, refreshAddr); code != 0 {
				rc = code
			}
		default:
			info, err := os.Stat(t)
			if err != nil {
				fmt.Fprintf(os.Stderr, "cmux open: not found: %s\n", t)
				rc = 1
				continue
			}
			if info.IsDir() {
				if code := emitV2(socketPath, "workspace.new",
					map[string]any{"directory": absPath(t)}, jsonOutput, refreshAddr); code != 0 {
					rc = code
				}
			} else {
				files = append(files, absPath(t))
			}
		}
	}
	if len(files) > 0 {
		if code := emitV2(socketPath, "file.open",
			map[string]any{"paths": files}, jsonOutput, refreshAddr); code != 0 {
			rc = code
		}
	}
	return rc
}

// firstPositionalOrCwd returns the first non-flag argument, or the remote cwd.
func firstPositionalOrCwd(args []string) string {
	for _, a := range args {
		if !strings.HasPrefix(a, "-") {
			return a
		}
	}
	return cwd()
}

func cwd() string {
	if wd, err := os.Getwd(); err == nil {
		return wd
	}
	return "."
}

// absPath resolves p to an absolute path, following symlinks when the target
// exists (like `realpath`), and otherwise returning the plain absolute path
// (like `realpath -m`). Runs on the remote host, so it yields a remote path.
func absPath(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		return p
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		return resolved
	}
	return abs
}
