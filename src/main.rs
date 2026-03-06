use std::collections::BTreeMap;
use yansi::Paint;
use zellij_tile::prelude::*;

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    List,
    Create,
    DeleteConfirm,
}

#[derive(Debug, Clone)]
struct WorktreeInfo {
    path: String,
    branch: Option<String>,
    is_current: bool,
}

struct State {
    mode: Mode,
    input: String,
    worktrees: Vec<WorktreeInfo>,
    selected_index: usize,
    error_message: Option<String>,
    waiting_for_command: bool,
    repo_root: Option<String>,
    base_path: Option<String>,
    initialized: bool,
    first_render: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: Mode::List,
            input: String::new(),
            worktrees: Vec::new(),
            selected_index: 0,
            error_message: None,
            waiting_for_command: false,
            repo_root: None,
            base_path: None,
            initialized: false,
            first_render: true,
        }
    }
}

register_plugin!(State);

impl State {
    fn parse_worktree_list(&mut self, output: &[u8]) {
        let output = String::from_utf8_lossy(output);
        // Use fully qualified syntax to avoid yansi's deprecated Paint::clear()
        Vec::clear(&mut self.worktrees);

        let mut current_path: Option<String> = None;

        for line in output.lines() {
            if let Some(new_current_path) = line.strip_prefix("worktree ") {
                current_path = Some(new_current_path.to_string());
            } else if let Some(path) = &current_path {
                if let Some(current_branch) = line.strip_prefix("branch ") {
                    let is_current = self.repo_root.as_ref().map(|p| p == path).unwrap_or(false);
                    self.worktrees.push(WorktreeInfo {
                        path: path.to_string(),
                        branch: Some(current_branch.to_string()),
                        is_current,
                    });
                    current_path = None;
                } else if line.starts_with("detached") {
                    let is_current = self.repo_root.as_ref().map(|p| p == path).unwrap_or(false);
                    self.worktrees.push(WorktreeInfo {
                        path: path.to_string(),
                        branch: None,
                        is_current,
                    });
                    current_path = None;
                }
            }
        }
        // Filter out the main worktree (usually first one)
        if !self.worktrees.is_empty() {
            self.worktrees.remove(0);
        }

        self.selected_index = 0;
    }

    fn resolve_worktree_path(&self, input: &str) -> Option<String> {
        // Absolute paths
        if input.starts_with('/') || input.starts_with('~') {
            return Some(input.to_string());
        }

        // Relative paths starting with ./ or ../
        if input.starts_with("./") || input.starts_with("../") {
            if let Some(repo_root) = &self.repo_root {
                let repo_path = std::path::Path::new(repo_root);
                return Some(repo_path.join(input).to_string_lossy().to_string());
            }
            return None;
        }

        // Branch names - create in base_path or parent directory
        if let Some(base_path) = &self.base_path {
            Some(format!("{}/{}", base_path, input))
        } else if let Some(repo_root) = &self.repo_root {
            let parent = std::path::Path::new(repo_root)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
            Some(format!("{}/{}", parent, input))
        } else {
            None
        }
    }

    fn get_tab_name(&self, path: &str) -> String {
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("worktree")
            .to_string()
    }

    fn clear_state(&mut self) {
        // Use fully qualified syntax to avoid yansi's deprecated Paint::clear()
        String::clear(&mut self.input);
        self.error_message = None;
        self.waiting_for_command = false;
        self.selected_index = 0;
    }

    fn refresh_git_info(&mut self) {
        self.initialized = false;
        self.repo_root = None;
        // Use fully qualified syntax to avoid yansi's deprecated Paint::clear()
        Vec::clear(&mut self.worktrees);
        self.error_message = None;
        self.waiting_for_command = true;
        self.mode = Mode::List;
        // Use fully qualified syntax to avoid yansi's deprecated Paint::clear()
        String::clear(&mut self.input);

        let mut context = BTreeMap::new();
        context.insert("command".to_string(), "rev-parse".to_string());
        run_command(&["git", "rev-parse", "--show-toplevel"], context);
    }
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
        ]);

        subscribe(&[
            EventType::Key,
            EventType::RunCommandResult,
            EventType::TabUpdate,
            EventType::Visible,
        ]);

        if let Some(base_path) = configuration.get("base_path") {
            self.base_path = Some(base_path.clone());
        }
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => {
                if self.waiting_for_command {
                    return false;
                }

                match key.bare_key {
                    BareKey::Esc if self.mode != Mode::List => {
                        self.mode = Mode::List;
                        self.clear_state();
                    }
                    BareKey::Esc => {
                        close_focus();
                    }
                    BareKey::Char('c') if key.has_modifiers(&[KeyModifier::Ctrl]) => {
                        close_focus();
                    }
                    BareKey::Enter => match self.mode {
                        Mode::List => {
                            if let Some(worktree) = self.worktrees.get(self.selected_index) {
                                let tab_name = self.get_tab_name(&worktree.path);
                                new_tab(Some(&tab_name), Some(&worktree.path));
                                close_focus();
                            }
                        }
                        Mode::Create => {
                            if !self.input.is_empty() {
                                if let Some(path) = self.resolve_worktree_path(&self.input) {
                                    let tab_name = self.get_tab_name(&path);
                                    self.waiting_for_command = true;
                                    self.error_message = None;

                                    let mut context = BTreeMap::new();
                                    context
                                        .insert("command".to_string(), "worktree-add".to_string());
                                    context.insert("tab_name".to_string(), tab_name);
                                    context.insert("path".to_string(), path.clone());
                                    run_command(&["git", "worktree", "add", &path], context);
                                } else {
                                    self.error_message = Some(
                                        [
                                            "Could not resolve path".to_string(),
                                            self.repo_root.clone().unwrap_or_default(),
                                        ]
                                        .concat(),
                                    );
                                }
                            }
                        }
                        Mode::DeleteConfirm => {
                            if let Some(worktree) = self.worktrees.get(self.selected_index) {
                                self.waiting_for_command = true;
                                self.error_message = None;

                                let mut context = BTreeMap::new();
                                context
                                    .insert("command".to_string(), "worktree-remove".to_string());
                                run_command(
                                    &["git", "worktree", "remove", &worktree.path],
                                    context,
                                );
                            }
                        }
                    },
                    BareKey::Backspace => {
                        if self.mode == Mode::Create {
                            self.input.pop();
                        }
                    }
                    BareKey::Char('n') if key.has_no_modifiers() && self.mode == Mode::List => {
                        self.mode = Mode::Create;
                        // Use fully qualified syntax to avoid yansi's deprecated Paint::clear()
                        String::clear(&mut self.input);
                        self.error_message = None;
                    }
                    BareKey::Char('d') if key.has_no_modifiers() && self.mode == Mode::List => {
                        if !self.worktrees.is_empty() && self.selected_index < self.worktrees.len()
                        {
                            self.mode = Mode::DeleteConfirm;
                        }
                    }
                    BareKey::Up | BareKey::Char('k')
                        if key.has_no_modifiers() && self.mode == Mode::List =>
                    {
                        if !self.worktrees.is_empty() {
                            if self.selected_index > 0 {
                                self.selected_index -= 1;
                            } else {
                                self.selected_index = self.worktrees.len() - 1;
                            }
                        }
                    }
                    BareKey::Down | BareKey::Char('j')
                        if key.has_no_modifiers() && self.mode == Mode::List =>
                    {
                        if !self.worktrees.is_empty() {
                            if self.selected_index < self.worktrees.len() - 1 {
                                self.selected_index += 1;
                            } else {
                                self.selected_index = 0;
                            }
                        }
                    }
                    BareKey::Char(c) if c.is_ascii() && key.has_no_modifiers() => {
                        if self.mode == Mode::Create {
                            self.input.push(c);
                        }
                    }
                    _ => {}
                }
                true
            }
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                let command_type = context.get("command").map(|s| s.as_str()).unwrap_or("");

                match command_type {
                    "rev-parse" => {
                        if exit_code == Some(0) {
                            let output = String::from_utf8_lossy(&stdout);
                            let path = output.trim().to_string();
                            if !path.is_empty() {
                                self.repo_root = Some(path);
                                let mut context = BTreeMap::new();
                                context.insert("command".to_string(), "worktree-list".to_string());
                                run_command(&["git", "worktree", "list", "--porcelain"], context);
                            } else {
                                self.waiting_for_command = false;
                                self.error_message =
                                    Some("Could not determine git root".to_string());
                            }
                        } else {
                            self.waiting_for_command = false;
                            self.error_message = Some("Not in a git repository".to_string());
                        }
                    }
                    "worktree-list" => {
                        self.parse_worktree_list(&stdout);
                        self.initialized = true;
                        self.waiting_for_command = false;
                    }
                    "worktree-add" => {
                        self.waiting_for_command = false;

                        match exit_code {
                            Some(0) => {
                                if let (Some(tab_name), Some(path)) =
                                    (context.get("tab_name"), context.get("path"))
                                {
                                    new_tab(Some(&tab_name), Some(&path));
                                    close_focus();
                                }
                            }
                            Some(code) => {
                                let error = String::from_utf8_lossy(&stderr);
                                self.error_message =
                                    Some(format!("Error ({}): {}", code, error.trim()));
                            }
                            None => {
                                self.error_message = Some("Command failed".to_string());
                            }
                        }
                    }
                    "worktree-remove" => {
                        self.waiting_for_command = false;

                        match exit_code {
                            Some(0) => {
                                self.mode = Mode::List;
                                self.clear_state();
                                let mut ctx = BTreeMap::new();
                                ctx.insert("command".to_string(), "worktree-list".to_string());
                                run_command(&["git", "worktree", "list", "--porcelain"], ctx);
                                self.waiting_for_command = true;
                            }
                            Some(code) => {
                                let error = String::from_utf8_lossy(&stderr);
                                self.error_message =
                                    Some(format!("Error ({}): {}", code, error.trim()));
                            }
                            None => {
                                self.error_message = Some("Command failed".to_string());
                            }
                        }
                    }
                    _ => {
                        // Unknown command type
                        self.waiting_for_command = false;
                    }
                }
                true
            }
            Event::TabUpdate(tabs) => {
                // Try to detect current worktree from focused tab
                if let Some(_focused_tab) = tabs.iter().find(|t| t.active) {
                    // We could use the tab's cwd if available
                    // For now, we rely on git rev-parse
                }
                false
            }
            Event::Visible(is_visible) => {
                if is_visible && !self.waiting_for_command {
                    // Refresh git info when plugin becomes visible
                    self.refresh_git_info();
                }
                true
            }
            _ => false,
        }
    }

    fn render(&mut self, _rows: usize, _cols: usize) {
        if self.first_render {
            self.first_render = false;
            self.refresh_git_info();
        }

        if !self.initialized {
            if let Some(error) = &self.error_message {
                println!("{}", error.red());
                println!();
                println!("{}", "Press Esc to close".bright_black());
            } else {
                println!("{}", "Loading...".yellow());
            }
            return;
        }

        match self.mode {
            Mode::List => {
                println!("{}", "Worktrees".cyan().bold());
                println!(
                    "{}",
                    "[j/k] navigate | [Enter] open | [n] new | [d] delete".bright_black()
                );
                println!();

                if self.worktrees.is_empty() {
                    println!("{}", "No worktrees found".bright_black());
                    println!();
                    println!("{}", "Press [n] to create a new worktree".bright_black());
                } else {
                    for (i, wt) in self.worktrees.iter().enumerate() {
                        let marker = if i == self.selected_index { ">" } else { " " };
                        let current = if wt.is_current {
                            format!(" {}", "(current)".yellow())
                        } else {
                            String::new()
                        };
                        let branch = wt.branch.as_deref().unwrap_or("detached");
                        let short_path = wt.path.split('/').next_back().unwrap_or(&wt.path);

                        println!("{} {} {} {}", marker, short_path, branch.cyan(), current);
                    }
                }

                if let Some(error) = &self.error_message {
                    println!();
                    println!("{}", error.red());
                }
            }
            Mode::Create => {
                println!("{}", "Create Worktree".cyan().bold());
                println!("{}", "[Esc] back to list".bright_black());
                println!();
                print!("Path/branch: {}", self.input);
                println!("{}", "_".blink());

                if let Some(error) = &self.error_message {
                    println!();
                    println!("{}", error.red());
                }

                if self.waiting_for_command {
                    println!();
                    println!("{}", "Creating worktree...".yellow());
                }
            }
            Mode::DeleteConfirm => {
                if let Some(wt) = self.worktrees.get(self.selected_index) {
                    println!("{}", "Confirm Delete".red().bold());
                    println!();
                    println!("Delete worktree: {}", wt.path.cyan());
                    if let Some(branch) = &wt.branch {
                        println!("Branch: {}", branch.yellow());
                    }
                    println!();
                    println!("{}", "[Enter] confirm | [Esc] cancel".bright_black());

                    if let Some(error) = &self.error_message {
                        println!();
                        println!("{}", error.red());
                    }

                    if self.waiting_for_command {
                        println!();
                        println!("{}", "Deleting...".yellow());
                    }
                }
            }
        }
    }
}
