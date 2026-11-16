-- Open a file from the GUI explorer, or dropped on window `win`. Uses a normal
-- file window (not a float, file tree or other special buffer). `:drop` switches
-- to the file's window if it is already open; `split` opens it beside the current
-- one.
local path, split, win = ...
if win and vim.api.nvim_win_is_valid(win) then
  vim.api.nvim_set_current_win(win)
end
local function usable(w)
  local buf = vim.api.nvim_win_get_buf(w)
  return vim.api.nvim_win_get_config(w).relative == '' and vim.bo[buf].buftype == ''
end
local editor = Eshvi.editor_window()
if editor and not usable(vim.api.nvim_get_current_win()) then
  vim.api.nvim_set_current_win(editor)
end
vim.cmd((split and editor and 'vsplit ' or 'drop ') .. vim.fn.fnameescape(path))
