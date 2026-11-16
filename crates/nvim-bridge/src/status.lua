-- What the native status bar shows (`[neovim] config = "own"` with
-- `[statusline] gui = true`): the mode, and the file of the window in use.
-- Sent as `gui_status { mode, path, name, filetype, buftype, buf, modified,
-- readonly }`. The GUI draws the bar itself; nothing here reads Neovim's
-- statusline.
--
-- The file part follows normal windows only: a float (a picker's prompt and
-- list, a hover) or a prompt buffer leaves the last file in place. The mode
-- is always current.
local chan = ...

local last_file = {
  path = '',
  name = '',
  filetype = '',
  buftype = '',
  buf = 0,
  modified = false,
  readonly = false,
  col = 0,
}
local last_sent = nil

local function describe(buf)
  local bt = vim.bo[buf].buftype
  local name = vim.api.nvim_buf_get_name(buf)
  local path = (bt == '' and name ~= '') and vim.fn.fnamemodify(name, ':p') or ''
  -- Help files, listings: the name as Neovim has it. Terminals and the
  -- quickfix list are named by the GUI.
  local short = ''
  if bt == '' or bt == 'help' or bt == 'nofile' or bt == 'acwrite' then
    short = vim.fn.fnamemodify(name, ':t')
  end
  local plain = bt == '' or bt == 'help'
  return {
    path = path,
    name = short,
    filetype = vim.bo[buf].filetype,
    buftype = bt,
    buf = buf,
    modified = plain and vim.bo[buf].modified or false,
    readonly = plain and (vim.bo[buf].readonly or not vim.bo[buf].modifiable) or false,
    -- In characters ("é" is one), which `win_viewport`'s byte column can't tell.
    col = vim.fn.charcol('.'),
  }
end

local function send()
  local win = vim.api.nvim_get_current_win()
  local buf = vim.api.nvim_win_get_buf(win)
  local normal = vim.api.nvim_win_get_config(win).relative == ''
  if normal and vim.bo[buf].buftype ~= 'prompt' and vim.fn.win_gettype(win) ~= 'autocmd' then
    last_file = describe(buf)
  end
  local payload = vim.tbl_extend('force', { mode = vim.api.nvim_get_mode().mode }, last_file)
  if vim.deep_equal(payload, last_sent) then
    return
  end
  last_sent = payload
  vim.rpcnotify(chan, 'gui_status', payload)
end

local group = vim.api.nvim_create_augroup('eshvi_status', { clear = true })
vim.api.nvim_create_autocmd(
  { 'ModeChanged', 'BufEnter', 'WinEnter', 'BufModifiedSet', 'FileType', 'BufFilePost', 'BufWritePost', 'CursorMoved', 'CursorMovedI' },
  { group = group, callback = send }
)
vim.api.nvim_create_autocmd('OptionSet', { group = group, pattern = { 'readonly', 'modifiable' }, callback = send })

-- One bar: the GUI draws a single status bar whatever the config says.
local function global()
  if vim.o.laststatus ~= 3 then
    vim.o.laststatus = 3
  end
end
vim.api.nvim_create_autocmd('OptionSet', { group = group, pattern = 'laststatus', callback = global })
if vim.v.vim_did_enter == 1 then
  global()
else
  vim.api.nvim_create_autocmd('VimEnter', { group = group, once = true, callback = global })
end

send()
