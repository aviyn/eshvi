-- Run by eshvi right after nvim_ui_attach. Arguments: the GUI's channel
-- id and an options table { sidebar_filetypes, global_statusline, winbar }.
local chan, opts = ...
opts = opts or {}

-- Clipboard: use Neovim's own provider (pbcopy, xclip, wl-copy, ...) when
-- one exists. Otherwise route "+ and "* through the GUI's clipboard.
-- Checked on VimEnter so a g:clipboard set in the user's config wins.
local function setup_clipboard()
  if vim.g.clipboard ~= nil then
    return
  end
  if vim.fn['provider#clipboard#Executable']() ~= '' then
    return
  end
  local function copy(lines, regtype)
    vim.rpcnotify(chan, 'eshvi_clipboard_set', lines, regtype)
  end
  local function paste()
    return vim.rpcrequest(chan, 'eshvi_clipboard_get')
  end
  vim.g.clipboard = {
    name = 'eshvi',
    copy = { ['+'] = copy, ['*'] = copy },
    paste = { ['+'] = paste, ['*'] = paste },
  }
end

if vim.v.vim_did_enter == 1 then
  setup_clipboard()
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = setup_clipboard })
end

-- ⌘C and ⌘X with no Visual selection (Edit › Copy is off then, so the key reaches the
-- window, and the GUI sends <D-c>, <D-x>): nothing. Unmapped, Neovim took <D-c> for `c`
-- (an operator waiting for its motion; which-key opened) and typed "<D-c>" in Insert mode.
-- (Text selected in a Markdown preview is copied by the GUI.) Not where the user's config
-- maps them.
local function quiet_cmd_keys()
  for _, key in ipairs({ '<D-c>', '<D-x>' }) do
    for _, mode in ipairs({ 'n', 'o', 'i', 'c' }) do
      if vim.fn.maparg(key, mode) == '' then
        vim.keymap.set(mode, key, '<Nop>', { desc = 'eshvi: nothing to copy' })
      end
    end
  end
end

if vim.v.vim_did_enter == 1 then
  quiet_cmd_keys()
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = quiet_cmd_keys })
end

-- Copy on select (`[neovim] copy_on_select`), as in the GUI's terminals: a Visual (or
-- Select) selection the mouse made goes on the clipboard when the button is let go (a
-- drag, a double or triple click). The GUI copies the text (`gui_copy`); it isn't a yank,
-- so `"0` and `""` aren't written; `"*` and `"+` read the clipboard, so they give it (and `p` does
-- with 'clipboard' unnamed or unnamedplus), as after Vim's 'autoselect'. A mapping of the user's for these keys
-- wins: defined before, it is kept; after, it replaces this one.
if opts.copy_on_select then
  local types = { v = 'v', V = 'V', ['\22'] = '\22', s = 'v', S = 'V', ['\19'] = '\22' }
  local function copy_selection()
    local kind = types[vim.fn.mode()]
    if not kind then
      return
    end
    local ok, lines = pcall(vim.fn.getregion, vim.fn.getpos('v'), vim.fn.getpos('.'), { type = kind })
    if ok and #lines > 0 then
      local text = table.concat(lines, '\n') .. (kind == 'V' and '\n' or '')
      if text ~= '' then
        vim.rpcnotify(chan, 'gui_copy', text)
      end
    end
  end
  for _, key in ipairs({ '<LeftRelease>', '<2-LeftRelease>', '<3-LeftRelease>', '<4-LeftRelease>' }) do
    for _, mode in ipairs({ 'x', 's' }) do
      if vim.fn.maparg(key, mode) == '' then
        vim.keymap.set(mode, key, function()
          -- (After the release itself has moved the selection's end.)
          vim.schedule(copy_selection)
          return key
        end, { expr = true, desc = 'eshvi: copy on select' })
      end
    end
  end
end

-- If the GUI dies while Neovim is blocked in a prompt (e.g. `:confirm qa`),
-- Neovim never notices its channel closed. Watch for being re-parented and
-- send ourselves SIGTERM, which preserves modified buffers in swap files.
local ppid = vim.uv.os_getppid()
local watchdog = vim.uv.new_timer()
watchdog:start(2000, 2000, function()
  if vim.uv.os_getppid() ~= ppid then
    watchdog:stop()
    vim.uv.kill(vim.uv.os_getpid(), 'sigterm')
  end
end)

-- Sidebar windows: report windows whose filetype is in the GUI's list
-- (second argument) so it can draw them as a styled side panel.
local sidebar_ft = {}
for _, ft in ipairs(opts.sidebar_filetypes or {}) do
  sidebar_ft[ft] = true
end
if next(sidebar_ft) then
  local last = nil
  local function report()
    local wins = {}
    for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
      if vim.api.nvim_win_is_valid(w) then
        local buf = vim.api.nvim_win_get_buf(w)
        if sidebar_ft[vim.bo[buf].filetype] then
          table.insert(wins, w)
        end
      end
    end
    table.sort(wins)
    local key = table.concat(wins, ',')
    if key ~= last then
      last = key
      vim.rpcnotify(chan, 'gui_sidebar', { wins = wins })
    end
  end
  vim.api.nvim_create_autocmd({ 'FileType', 'BufWinEnter', 'WinNew', 'WinClosed', 'TabEnter', 'VimEnter' }, {
    group = vim.api.nvim_create_augroup('eshvi_sidebar', { clear = true }),
    callback = function()
      -- WinClosed fires before the window is gone.
      vim.schedule(report)
    end,
  })
end

-- LSP progress for the GUI's status bar: the latest running `$/progress`
-- of any client, or nil once none is. (Neovim's LSP client fires
-- LspProgress but creates no progress message, so the native progress bar,
-- OSC 9;4 for |Progress| events, never sees LSP work.)
do
  local running, seq = {}, 0
  local group = vim.api.nvim_create_augroup('eshvi_progress', { clear = true })
  local function report()
    local latest
    for _, p in pairs(running) do
      if not latest or p.seq > latest.seq then
        latest = p
      end
    end
    vim.rpcnotify(chan, 'gui_progress', latest or vim.NIL)
  end
  -- A server that stops mid-way never ends its progress.
  vim.api.nvim_create_autocmd('LspDetach', {
    group = group,
    callback = function(ev)
      local prefix = tostring(ev.data.client_id) .. ':'
      for key in pairs(running) do
        if vim.startswith(key, prefix) then
          running[key] = nil
        end
      end
      report()
    end,
  })
  vim.api.nvim_create_autocmd('LspProgress', {
    group = group,
    callback = function(ev)
      local params = ev.data and ev.data.params or {}
      local value = params.value or {}
      local key = tostring(ev.data.client_id) .. ':' .. tostring(params.token)
      if value.kind == 'end' then
        running[key] = nil
      else
        seq = seq + 1
        local client = vim.lsp.get_client_by_id(ev.data.client_id)
        local prev = running[key] or {}
        running[key] = {
          seq = seq,
          source = client and client.name or 'LSP',
          title = value.title or prev.title or '',
          message = value.message or '',
          percent = value.percentage or vim.NIL,
        }
      end
      report()
    end,
  })
end

-- Patch a plugin module when it loads (or now, if it has), leaving the
-- user's config alone.
local function on_require(name, patch)
  if package.loaded[name] then
    patch(package.loaded[name])
    return
  end
  package.preload[name] = function()
    package.preload[name] = nil
    -- The searchers after preload (the plugin manager's among them).
    for i = 2, #package.loaders do
      local loader = package.loaders[i](name)
      if type(loader) == 'function' then
        local mod = loader(name)
        patch(mod)
        return mod
      end
    end
    error('module ' .. name .. ' not found')
  end
end

-- which-key: the title of its window (drawn as the frame's header) is the
-- keys pressed so far, "Space f g", instead of which-key's breadcrumb of
-- group names ("␣ » +file/find"), which the list below shows anyway.
local key_names = { Space = 'Space', CR = 'Return', Tab = 'Tab', BS = 'Backspace', Esc = 'Esc', lt = '<', Bslash = '\\', Bar = '|' }
local key_mods = { C = 'Ctrl', M = 'Alt', A = 'Alt', S = 'Shift', D = 'Cmd' }
local function key_label(key)
  local inner = key:match('^<(.+)>$')
  if not inner then
    return key == ' ' and 'Space' or key
  end
  local mods, name = inner:match('^(.-)%-?([^-]+%-?)$')
  if not name then
    return key
  end
  local out = {}
  for m in (mods or ''):gmatch('[^-]+') do
    table.insert(out, key_mods[m:upper()] or m)
  end
  name = key_names[name] or name
  table.insert(out, #out > 0 and #name == 1 and name:upper() or name)
  return table.concat(out, '+')
end
on_require('which-key.view', function(view)
  local trail = view.trail
  view.trail = function(node, o)
    if o and o.title and node and node.path and #node.path > 0 then
      return { { ' ' .. table.concat(vim.tbl_map(key_label, node.path), ' ') .. ' ', 'WhichKeyTitle' } }
    end
    return trail(node, o)
  end
end)

-- snacks.image: an image buffer's placement is hidden when the buffer
-- leaves its last window, and nothing shows it again when the buffer comes
-- back (`:b`, `:drop`, the explorer), so the image stays blank (with the
-- conversion spinner left under it, if it was still converting). Unhide it
-- whenever a window shows the buffer again, and drop the stale spinner.
-- Inline images (markdown) show and hide themselves; they're left alone.
on_require('snacks.image.placement', function(placement)
  local update = placement.update
  local ns = vim.api.nvim_create_namespace('snacks.image')
  placement.update = function(self, ...)
    if self.opts.inline or not self:ready() then
      return update(self, ...)
    end
    if self.hidden and #self:wins() > 0 then
      self.hidden = false
    end
    local result = update(self, ...)
    local own = {}
    for _, id in ipairs(self.eids) do
      own[id] = true
    end
    for _, mark in ipairs(vim.api.nvim_buf_get_extmarks(self.buf, ns, 0, -1, {})) do
      if not own[mark[1]] then
        vim.api.nvim_buf_del_extmark(self.buf, ns, mark[1])
      end
    end
    return result
  end
end)

-- The right-click menu (PopUp): the GUI draws it like its other menus.
-- Neovim has drawn its own by then and waits for a choice, which the GUI
-- makes by clicking that menu's row (or cancels with Esc). The items are those
-- Neovim shows: separators, and the items enabled in the current mode
-- (after its own MenuPopup handler has enabled or disabled them).
local menu_modes = { n = 'n', v = 'v', V = 'v', ['\22'] = 'v', s = 's', S = 's', ['\19'] = 's', i = 'i', R = 'i', c = 'c', t = 't' }
vim.api.nvim_create_autocmd('MenuPopup', {
  group = vim.api.nvim_create_augroup('eshvi_popup_menu', { clear = true }),
  callback = function()
    local mode = menu_modes[vim.fn.mode():sub(1, 1)] or 'n'
    local popup = vim.fn.menu_get('PopUp', mode)[1]
    if not popup then
      return
    end
    local items = {}
    for _, sub in ipairs(popup.submenus or {}) do
      local sep = sub.name:match('^%-.*%-$')
      local map = (sub.mappings or {})[mode]
      if sep or (map and map.enabled ~= 0) then
        table.insert(items, { name = sub.name, key = sub.actext or '' })
      end
    end
    vim.rpcnotify(chan, 'gui_popup_menu', { items = items })
  end,
})

-- 'laststatus' decides whether statuslines are per window or global.
local function report_options()
  vim.rpcnotify(chan, 'gui_options', { laststatus = vim.o.laststatus })
end
vim.api.nvim_create_autocmd('OptionSet', { pattern = 'laststatus', callback = report_options })
vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = report_options })
report_options()

-- Layout inside eshvi: one global statusline (thin separators between
-- windows) and a filename winbar on normal file windows. With the global
-- statusline, a window below another has none: the GUI draws its file tab
-- in the separator row above it (else a blank row, then the tab).
local function apply_layout()
  if opts.global_statusline then
    vim.o.laststatus = 3
  end
  if opts.winbar then
    local wins = vim.tbl_filter(function(w)
      return vim.api.nvim_win_get_config(w).relative == ''
    end, vim.api.nvim_tabpage_list_wins(0))
    local top = math.huge
    for _, w in ipairs(wins) do
      top = math.min(top, vim.api.nvim_win_get_position(w)[1])
    end
    for _, w in ipairs(wins) do
      local buf = vim.api.nvim_win_get_buf(w)
      local below = vim.o.laststatus == 3 and vim.api.nvim_win_get_position(w)[1] > top
      -- (A diff buffer takes a file's place in its window, header included.)
      local file = vim.bo[buf].buftype == '' or vim.b[buf].eshvi_diff_prev ~= nil
      local normal = not below and file and not sidebar_ft[vim.bo[buf].filetype]
      if normal and vim.wo[w].winbar == '' then
        vim.wo[w].winbar = '%f %m'
      elseif not normal and vim.wo[w].winbar == '%f %m' then
        vim.wo[w].winbar = ''
      end
    end
  end
end
if opts.global_statusline or opts.winbar then
  local group = vim.api.nvim_create_augroup('eshvi_layout', { clear = true })
  vim.api.nvim_create_autocmd({ 'VimEnter', 'BufWinEnter', 'WinNew', 'WinClosed', 'WinResized', 'FileType', 'TabEnter' }, {
    group = group,
    callback = function()
      vim.schedule(apply_layout)
    end,
  })
  -- Lazy-loading plugin managers load more plugins (statuslines) later.
  vim.api.nvim_create_autocmd('User', { pattern = 'VeryLazy', group = group, callback = apply_layout })
  if vim.v.vim_did_enter == 1 then
    apply_layout()
  end
end

-- The colorscheme's ANSI colors for the GUI's terminals: g:terminal_color_0..15
-- (-1 where a scheme sets none), at start and on every ColorScheme.
local function report_term_colors()
  local colors = {}
  for i = 0, 15 do
    local c = vim.g['terminal_color_' .. i]
    colors[i + 1] = type(c) == 'string' and tonumber(c:gsub('^#', ''), 16) or -1
  end
  vim.rpcnotify(chan, 'gui_term_colors', colors)
end
vim.api.nvim_create_autocmd('ColorScheme', { callback = report_term_colors })
if vim.v.vim_did_enter == 1 then
  vim.schedule(report_term_colors)
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = report_term_colors })
end

-- Native explorer: report explicit working directory changes (`:cd`, not
-- 'autochdir' following windows) and the current file, and provide
-- :EshviExplorer [toggle|focus|reveal|show|hide] (and :EshviGit below).
local function report_cwd()
  if vim.v.event.changed_window then
    return
  end
  vim.rpcnotify(chan, 'gui_cwd', { cwd = vim.fn.getcwd(-1, -1) })
end
local function report_buffer()
  -- Not a buffer loaded in the background (bufload(), a plugin's preview):
  -- its autocommands run in a hidden "autocmd" window where it is current.
  if vim.fn.win_gettype() == 'autocmd' then
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  if vim.bo[buf].buftype == '' then
    local name = vim.api.nvim_buf_get_name(buf)
    vim.rpcnotify(chan, 'gui_buffer', {
      path = name ~= '' and vim.fn.fnamemodify(name, ':p') or '',
      filetype = vim.bo[buf].filetype,
    })
  end
end
local explorer_group = vim.api.nvim_create_augroup('eshvi_explorer', { clear = true })
vim.api.nvim_create_autocmd('DirChanged', { group = explorer_group, pattern = 'global', callback = report_cwd })
vim.api.nvim_create_autocmd({ 'VimEnter', 'BufEnter', 'BufFilePost', 'FileType' }, { group = explorer_group, callback = report_buffer })
report_buffer()
vim.api.nvim_create_user_command('EshviExplorer', function(o)
  vim.rpcnotify(chan, 'gui_explorer', { action = o.args ~= '' and o.args or 'toggle' })
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'focus', 'reveal', 'show', 'hide' }
  end,
  desc = 'eshvi native file explorer',
})
-- :EshviTermDebug [toggle|show|hide|focus]: a floating window with one of the GUI's own
-- terminals (a login shell in the explorer's folder), to try them out. Hidden: no menu item.
vim.api.nvim_create_user_command('EshviTermDebug', function(o)
  vim.rpcnotify(chan, 'gui_term_debug', { action = o.args ~= '' and o.args or 'toggle' })
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'show', 'hide', 'focus' }
  end,
  desc = 'eshvi native terminal debug window',
})
-- :EshviGit [toggle|focus|show|hide|graph]: the Source Control panel, which takes
-- the explorer's place in the side bar. `graph` unfolds the Graph and focuses the
-- panel: the Graph takes no keys of its own.
vim.api.nvim_create_user_command('EshviGit', function(o)
  vim.rpcnotify(chan, 'gui_git', { action = o.args ~= '' and o.args or 'toggle' })
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'focus', 'show', 'hide', 'graph' }
  end,
  desc = 'eshvi native source control',
})

-- :EshviOutline [toggle|focus|show|hide]: the Outline pane at the foot of the
-- Explorer (`outline.lua` fills it). `focus` unfolds it and gives it the keyboard.
vim.api.nvim_create_user_command('EshviOutline', function(o)
  vim.rpcnotify(chan, 'gui_outline', { action = o.args ~= '' and o.args or 'toggle' })
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'focus', 'show', 'hide' }
  end,
  desc = 'eshvi native Outline',
})

-- :EshviTree {show|focus|hide|toggle} {view}: a tree view with a place of its
-- own in the side bar (`Eshvi.tree.register(view, { side = ... })`).
vim.api.nvim_create_user_command('EshviTree', function(o)
  local action, view = o.fargs[1], o.fargs[2]
  if not (action and view) then
    vim.notify('EshviTree {show|focus|hide|toggle} {view}', vim.log.levels.ERROR)
    return
  end
  Eshvi.tree.command(action, view)
end, {
  nargs = '+',
  complete = function(lead, line)
    local names = #vim.split(line, '%s+') <= 2 and { 'show', 'focus', 'hide', 'toggle' } or Eshvi.tree.names()
    table.sort(names)
    return vim.tbl_filter(function(name)
      return vim.startswith(name, lead)
    end, names)
  end,
  desc = 'eshvi native tree view in the side bar',
})

-- Syntax colors of the colorscheme, for code shown in the GUI's popups
-- (hover markdown): for each role, the first group that has a foreground.
local syntax_roles = {
  text = { '@variable', 'Normal' },
  comment = { '@comment', 'Comment' },
  string = { '@string', 'String' },
  escape = { '@string.escape', 'SpecialChar' },
  number = { '@number', 'Number' },
  constant = { '@constant.builtin', '@boolean', 'Constant' },
  keyword = { '@keyword', 'Keyword', 'Statement' },
  operator = { '@operator', 'Operator' },
  ['function'] = { '@function', 'Function' },
  macro = { '@function.macro', 'Macro' },
  type = { '@type', 'Type' },
  property = { '@variable.member', '@property', 'Identifier' },
  parameter = { '@variable.parameter', 'Identifier' },
  module = { '@module', 'Include' },
  attribute = { '@attribute', 'PreProc' },
}
local function report_syntax()
  local colors = {}
  for role, groups in pairs(syntax_roles) do
    for _, group in ipairs(groups) do
      local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
      if hl.fg then
        colors[role] = { fg = hl.fg, italic = hl.italic or false, bold = hl.bold or false }
        break
      end
    end
  end
  vim.rpcnotify(chan, 'gui_syntax', colors)
end
vim.api.nvim_create_autocmd('ColorScheme', { callback = report_syntax })
if vim.v.vim_did_enter == 1 then
  vim.schedule(report_syntax)
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = report_syntax })
end

-- Decoration colors of the colorscheme (git status and diagnostics, for the
-- explorer, tabs and panels): for each role, the first group that has a
-- foreground. Separate from the syntax colors: plain numbers, not styles.
local deco_roles = {
  added = { 'GitSignsAdd', 'Added', 'diffAdded', 'DiffAdd' },
  modified = { 'GitSignsChange', 'Changed', 'diffChanged', 'DiffChange' },
  deleted = { 'GitSignsDelete', 'Removed', 'diffRemoved', 'DiffDelete' },
  error = { 'DiagnosticError' },
  warn = { 'DiagnosticWarn' },
  info = { 'DiagnosticInfo' },
}
local function report_deco_colors()
  local colors = {}
  for role, groups in pairs(deco_roles) do
    for _, group in ipairs(groups) do
      local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
      if hl.fg then
        colors[role] = hl.fg
        break
      end
    end
  end
  vim.rpcnotify(chan, 'gui_deco_colors', colors)
end
vim.api.nvim_create_autocmd('ColorScheme', { callback = report_deco_colors })
if vim.v.vim_did_enter == 1 then
  vim.schedule(report_deco_colors)
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = report_deco_colors })
end

-- The colorscheme's backgrounds for the GUI's diff view: `DiffAdd`,
-- `DiffDelete`, `DiffChange` and `DiffText` (a group with no background is
-- left out; the GUI tints the git colors then).
local diff_roles = { add = 'DiffAdd', delete = 'DiffDelete', change = 'DiffChange', text = 'DiffText' }
local function report_diff_colors()
  local colors = {}
  for role, group in pairs(diff_roles) do
    local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
    if hl.bg then
      colors[role] = hl.bg
    end
  end
  vim.rpcnotify(chan, 'gui_diff_colors', colors)
end
vim.api.nvim_create_autocmd('ColorScheme', { callback = report_diff_colors })
if vim.v.vim_did_enter == 1 then
  vim.schedule(report_diff_colors)
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = report_diff_colors })
end

-- Diagnostics summary: error/warning counts per file, for the status bar,
-- explorer and tabs. Debounced; sent whenever diagnostics change.
local diag_timer = vim.uv.new_timer()
local function report_diagnostics()
  local per = {}
  for _, d in ipairs(vim.diagnostic.get(nil)) do
    if vim.api.nvim_buf_is_valid(d.bufnr) then
      local name = vim.api.nvim_buf_get_name(d.bufnr)
      if name ~= '' then
        local path = vim.fn.fnamemodify(name, ':p')
        local c = per[path] or { errors = 0, warnings = 0 }
        if d.severity == vim.diagnostic.severity.ERROR then
          c.errors = c.errors + 1
        elseif d.severity == vim.diagnostic.severity.WARN then
          c.warnings = c.warnings + 1
        end
        per[path] = c
      end
    end
  end
  local files = {}
  for path, c in pairs(per) do
    if c.errors + c.warnings > 0 then
      table.insert(files, { path = path, errors = c.errors, warnings = c.warnings })
    end
  end
  vim.rpcnotify(chan, 'gui_diagnostics', { files = files })
end
vim.api.nvim_create_autocmd('DiagnosticChanged', {
  group = explorer_group,
  callback = function()
    diag_timer:stop()
    diag_timer:start(150, 0, vim.schedule_wrap(report_diagnostics))
  end,
})

-- Which file each window shows (tabs use real paths, not winbar text).
local last_windows = nil
local function report_windows()
  local wins = {}
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_is_valid(w) then
      local buf = vim.api.nvim_win_get_buf(w)
      local name = vim.api.nvim_buf_get_name(buf)
      if vim.bo[buf].buftype == '' and name ~= '' then
        table.insert(wins, { win = w, path = vim.fn.fnamemodify(name, ':p'), modified = vim.bo[buf].modified })
      end
    end
  end
  local key = vim.inspect(wins)
  if key ~= last_windows then
    last_windows = key
    vim.rpcnotify(chan, 'gui_windows', { wins = wins })
  end
end
vim.api.nvim_create_autocmd({ 'BufWinEnter', 'WinNew', 'WinClosed', 'TabEnter', 'BufFilePost', 'VimEnter', 'BufModifiedSet' }, {
  group = explorer_group,
  callback = function()
    vim.schedule(report_windows)
  end,
})
report_windows()

-- Saves: the GUI's file watcher only sees folders open in the explorer, so
-- it hears of them here too (the Changes count, git decorations).
vim.api.nvim_create_autocmd('BufWritePost', {
  group = explorer_group,
  callback = function(a)
    vim.rpcnotify(chan, 'gui_saved', vim.fn.fnamemodify(a.match, ':p'))
  end,
})

_G.Eshvi = _G.Eshvi or {}

-- A completion item's documentation as Markdown (an LSP item: `detail` in a
-- code block, `---`, `documentation`), for blink_docs.lua and completion.lua.
function Eshvi.item_markdown(item)
  local lines = {}
  local doc = item.documentation
  if type(doc) == 'string' then
    doc = { kind = 'plaintext', value = doc }
  end
  if type(doc) == 'table' and doc.value then
    vim.lsp.util.convert_input_to_markdown_lines(doc, lines)
  end
  local body = vim.trim(table.concat(lines, '\n'))
  local detail = type(item.detail) == 'string' and vim.trim(item.detail) or ''
  if detail == '' or body:find(detail, 1, true) then
    return body
  end
  local code = '```' .. vim.bo.filetype .. '\n' .. detail .. '\n```'
  return body == '' and code or code .. '\n\n---\n\n' .. body
end

-- Scroll window `win` so line `top` (1-based) is first in view, for the
-- GUI's scrollbars. The cursor comes along (within 'scrolloff'), as with the
-- mouse wheel: left outside the view, Neovim would scroll back to it.
local function scroll_current_to(top)
  local h = vim.api.nvim_win_get_height(0)
  local last = vim.fn.line('$')
  local so = math.min(vim.wo.scrolloff, math.floor((h - 1) / 2))
  local lo = top == 1 and 1 or top + so
  local hi = top + h - 1 >= last and last or top + h - 1 - so
  local view = vim.fn.winsaveview()
  view.topline = top
  view.lnum = math.max(lo, math.min(hi, view.lnum))
  vim.fn.winrestview(view)
end

-- The window to scroll for `win`: the sides of a diff are bound
-- ('scrollbind'), and the others follow the current window only (a side that
-- isn't current, scrolled, is put back), so the current side is scrolled
-- and the other follows. `win` itself when it is the current one or isn't
-- in a diff. (`scroll_to` maps `top`, a line of `win`'s buffer, to the
-- other side's by position in the file.)
local function scroll_master(win)
  local cur = vim.api.nvim_get_current_win()
  if cur ~= win and vim.wo[win].diff and vim.wo[win].scrollbind and vim.wo[cur].diff and vim.wo[cur].scrollbind
    and vim.api.nvim_win_get_tabpage(cur) == vim.api.nvim_win_get_tabpage(win) then
    return cur
  end
  return win
end

function Eshvi.scroll_to(win, top)
  local master = scroll_master(win)
  if master ~= win then
    local mine = vim.api.nvim_buf_line_count(vim.api.nvim_win_get_buf(win))
    local theirs = vim.api.nvim_buf_line_count(vim.api.nvim_win_get_buf(master))
    top = math.max(math.floor((top - 1) * theirs / math.max(mine, 1)) + 1, 1)
  end
  pcall(vim.api.nvim_win_call, master, function()
    scroll_current_to(top)
  end)
end

-- The wheel over a file window: `rows` lines down (up if negative), by the
-- same path as dragging its scroll bar. A closed fold counts as one line.
function Eshvi.scroll_by(win, rows)
  pcall(vim.api.nvim_win_call, scroll_master(win), function()
    local top = vim.fn.winsaveview().topline
    local last = vim.fn.line('$')
    for _ = 1, math.abs(rows) do
      if rows > 0 then
        local stop = vim.fn.foldclosedend(top)
        top = math.min((stop > 0 and stop or top) + 1, last)
      else
        top = math.max(top - 1, 1)
        local start = vim.fn.foldclosed(top)
        top = start > 0 and start or top
      end
    end
    scroll_current_to(top)
  end)
end

-- The window to show a file in, opened from the GUI (a panel, the explorer): the last used
-- of those holding a file, else of those holding something a file can replace
-- (a start screen, oil, a diff buffer; what `:e` there would replace too). Not
-- a float, nor a plugin's side bar or list, which fix their size.
function Eshvi.editor_window()
  local previous = vim.fn.win_getid(vim.fn.winnr('#'))
  local file, other
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    local buftype = vim.bo[vim.api.nvim_win_get_buf(w)].buftype
    if vim.api.nvim_win_get_config(w).relative == '' and not vim.wo[w].winfixwidth and not vim.wo[w].winfixheight then
      if buftype == '' then
        file = (file == nil or w == previous) and w or file
      elseif buftype == 'nofile' or buftype == 'acwrite' or buftype == 'nowrite' then
        other = (other == nil or w == previous) and w or other
      end
    end
  end
  return file or other
end

-- ⌃h/j/k/l, tmux style (the config maps them): move to the neighbouring window; at the
-- edge of the windows (nothing moved), tell the GUI, which gives the keyboard to the region
-- beyond (the side bar, the bottom panel).
function Eshvi.navigate(dir)
  local win = vim.api.nvim_get_current_win()
  vim.cmd('wincmd ' .. dir)
  if vim.api.nvim_get_current_win() == win then
    vim.rpcnotify(chan, 'gui_navigate', { dir = dir })
  end
end

-- A diff buffer (`b:eshvi_diff_prev`, diff.lua) in window `win` goes back to
-- what the window showed before it. Whether there was one.
function Eshvi.close_diff(win)
  local prev = vim.b[vim.api.nvim_win_get_buf(win)].eshvi_diff_prev
  if not prev then
    return false
  end
  if vim.api.nvim_buf_is_valid(prev) then
    vim.api.nvim_win_set_buf(win, prev)
  else
    vim.api.nvim_win_call(win, function()
      vim.cmd('enew')
    end)
  end
  return true
end

-- A window file tab's "×" (or a middle click on it): close a diff buffer, or
-- window `win` as :q does (with 'hidden', Neovim's default, a modified buffer
-- stays, hidden, with its changes).
-- The last window showing a file instead shows an empty buffer: closing it
-- would leave only non-file windows or quit.
function Eshvi.close_window(win)
  if not vim.api.nvim_win_is_valid(win) or Eshvi.close_diff(win) then
    return
  end
  -- A preview beside its file just closes.
  if vim.w[win].eshvi_preview_of then
    pcall(vim.api.nvim_win_close, win, true)
    return
  end
  local files = 0
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(vim.api.nvim_win_get_tabpage(win))) do
    if vim.api.nvim_win_get_config(w).relative == '' and vim.bo[vim.api.nvim_win_get_buf(w)].buftype == '' then
      files = files + 1
    end
  end
  vim.api.nvim_set_current_win(win)
  local ok, err = pcall(vim.cmd, files > 1 and 'quit' or 'enew')
  if not ok then
    vim.notify((tostring(err):gsub('^.-:E', 'E')), vim.log.levels.ERROR)
  end
end
