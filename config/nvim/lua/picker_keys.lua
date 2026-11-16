-- The keys of Eshvi's native picker (`Eshvi.picker`, defined by the app when it
-- runs Neovim with `[neovim] config = "own"`), and the views the git keys open.
-- They used to be snacks.nvim's `keys`; snacks' picker is off now.
-- Later lines win over earlier ones on the same key, as they did in snacks' spec.
-- The extensions `options.lua` gives the systemverilog filetype.
local verilog = { "v", "vh", "sv", "svh", "svp", "svi" }

local keys = {
  { "<leader><space>", function() Eshvi.picker.smart() end, desc = "Smart Find Files" },
  { "<leader>,", function() Eshvi.picker.buffers() end, desc = "Buffers" },
  { "<leader>/", function() Eshvi.picker.grep() end, desc = "Grep" },
  { "<leader>:", function() Eshvi.picker.command_history() end, desc = "Command History" },
  -- find
  { "<leader>fb", function() Eshvi.picker.buffers() end, desc = "Buffers" },
  { "<leader>fc", function() Eshvi.picker.files({ cwd = vim.fn.stdpath("config") }) end, desc = "Find Config File" },
  { "<leader>ff", function() Eshvi.picker.files() end, desc = "Find Files" },
  { "<leader>fg", function() Eshvi.picker.git_files() end, desc = "Find Git Files" },
  { "<leader>fr", function() Eshvi.picker.recent() end, desc = "Recent" },
  { "<leader>fe", "<cmd>EshviExplorer focus<cr>", desc = "File Explorer" },
  -- git: the panel and its Graph are native views; a file's and a line's history are pickers
  { "<leader>gl", "<cmd>EshviGit graph<cr>", desc = "Git Log" },
  { "<leader>gL", function() Eshvi.picker.git_log_line() end, desc = "Git Log Line" },
  { "<leader>gs", "<cmd>EshviGit focus<cr>", desc = "Git Status" },
  { "<leader>gd", "<cmd>EshviGit focus<cr>", desc = "Git Diff" },
  { "<leader>gf", function() Eshvi.picker.git_log_file() end, desc = "Git Log File" },
  -- Grep
  { "<leader>sb", function() Eshvi.picker.lines() end, desc = "Buffer Lines" },
  { "<leader>sB", function() Eshvi.picker.grep_buffers() end, desc = "Grep Open Buffers" },
  { "<leader>sg", function() Eshvi.picker.grep() end, desc = "Grep" },
  { "<leader>sw", function() Eshvi.picker.grep_word() end, desc = "Visual selection or word", mode = { "n", "x" } },
  { "<leader>vg", function() Eshvi.picker.grep({ title = "Grep Verilog", extensions = verilog }) end, desc = "Grep Verilog" },
  { "<leader>vw", function() Eshvi.picker.grep_word({ title = "Grep Verilog word", extensions = verilog }) end, desc = "Grep Verilog: word or selection", mode = { "n", "x" } },
  -- search
  { '<leader>s"', function() Eshvi.picker.registers() end, desc = "Registers" },
  { "<leader>s/", function() Eshvi.picker.search_history() end, desc = "Search History" },
  { "<leader>sc", function() Eshvi.picker.command_history() end, desc = "Command History" },
  { "<leader>sC", function() Eshvi.picker.commands() end, desc = "Commands" },
  { "<leader>sd", function() Eshvi.picker.diagnostics() end, desc = "Diagnostics" },
  { "<leader>sd", function() Eshvi.picker.diagnostics({ buf = true }) end, desc = "Buffer Diagnostics" },
  { "<leader>sh", function() Eshvi.picker.help() end, desc = "Help Pages" },
  { "<leader>sj", function() Eshvi.picker.jumps() end, desc = "Jumps" },
  { "<leader>sk", function() Eshvi.picker.keymaps() end, desc = "Keymaps" },
  { "<leader>sl", function() Eshvi.picker.loclist() end, desc = "Location List" },
  { "<leader>sm", function() Eshvi.picker.marks() end, desc = "Marks" },
  { "<leader>sq", function() Eshvi.picker.qflist() end, desc = "Quickfix List" },
  { "<leader>sR", function() Eshvi.picker.resume() end, desc = "Resume" },
  { "<leader>su", function() Eshvi.picker.undo() end, desc = "Undo History" },
  { "<leader>uC", function() Eshvi.picker.colorschemes() end, desc = "Colorschemes" },
  -- LSP
  { "gd", function() Eshvi.picker.lsp_definitions() end, desc = "Goto Definition" },
  { "gD", function() Eshvi.picker.lsp_declarations() end, desc = "Goto Declaration" },
  { "gr", function() Eshvi.picker.lsp_references() end, nowait = true, desc = "References" },
  { "gI", function() Eshvi.picker.lsp_implementations() end, desc = "Goto Implementation" },
  { "gy", function() Eshvi.picker.lsp_type_definitions() end, desc = "Goto T[y]pe Definition" },
  { "<leader>ss", function() Eshvi.picker.lsp_symbols() end, desc = "LSP Symbols" },
  { "<leader>sS", function() Eshvi.picker.lsp_workspace_symbols() end, desc = "LSP Workspace Symbols" },
}

for _, key in ipairs(keys) do
  vim.keymap.set(key.mode or "n", key[1], key[2], { desc = key.desc, nowait = key.nowait, silent = true })
end

return keys
