-- eshvi's menus, defined with Neovim's own :menu (as MacVim does): the GUI
-- mirrors every menu — these and the user's own `:amenu` items — as the
-- native menu bar. The text after <Tab> is the shortcut, in macOS notation
-- (⌘S, ⇧⌘S, ⌥⌘S, ⌃`, F12, ⇧F12).
--
-- Items run `<Cmd>lua Eshvi.menu('action')<CR>` in every mode, so an action
-- sees the mode it was chosen in (Copy keeps a visual selection).
local chan, opts = ...

_G.Eshvi = _G.Eshvi or {}

local function gui(action)
  vim.rpcnotify(chan, 'gui_action', action)
end

local function feed(keys)
  vim.api.nvim_feedkeys(vim.keycode(keys), 'n', false)
end

local function visual()
  return vim.fn.mode():match('^[vVsS\22\19]') ~= nil
end

-- Leave insert/terminal/visual mode first (for keys typed on the user's
-- behalf).
local function to_normal()
  if vim.fn.mode() ~= 'n' then
    feed('<C-\\><C-n>')
  end
end

-- A window showing a file: the current one, else the previous, else any.
local function editor_win()
  local function usable(w)
    return vim.api.nvim_win_is_valid(w)
      and vim.api.nvim_win_get_config(w).relative == ''
      and vim.bo[vim.api.nvim_win_get_buf(w)].buftype == ''
  end
  local cur, prev = vim.api.nvim_get_current_win(), vim.fn.win_getid(vim.fn.winnr('#'))
  for _, w in ipairs({ cur, prev }) do
    if usable(w) then
      return w
    end
  end
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if usable(w) then
      return w
    end
  end
end

-- Run `f` in the editor window (a file command chosen while a panel or the
-- sidebar has the keyboard acts on the file being edited).
local function in_editor(f)
  local w = editor_win()
  if w then
    vim.api.nvim_set_current_win(w)
  end
  if vim.fn.mode() ~= 'n' and vim.fn.mode() ~= 'i' then
    vim.cmd('stopinsert')
  end
  f()
end

local function cmd(c)
  return function()
    local ok, err = pcall(vim.cmd, c)
    if not ok then
      vim.notify(tostring(err):gsub('^Vim%(%w+%):', ''), vim.log.levels.ERROR)
    end
  end
end

-- Close the file in the current window but keep the window (VS Code's
-- Close Editor): show another buffer first, then delete it. Special
-- windows (a terminal, help, the quickfix list) close.
local function close_editor()
  local bt = vim.bo.buftype
  if bt ~= '' then
    return pcall(vim.cmd, 'close')
  end
  local buf = vim.api.nvim_get_current_buf()
  local alt = vim.fn.bufnr('#')
  if alt > 0 and alt ~= buf and vim.fn.buflisted(alt) == 1 then
    vim.cmd('buffer #')
  else
    local others = vim.tbl_filter(function(b)
      return b.bufnr ~= buf
    end, vim.fn.getbufinfo({ buflisted = 1 }))
    if #others > 0 then
      vim.cmd('buffer ' .. others[1].bufnr)
    else
      vim.cmd('enew')
    end
  end
  pcall(vim.cmd, 'confirm bdelete ' .. buf)
end

local actions = {
  -- File
  new_file = function()
    in_editor(cmd('enew'))
  end,
  open_file = function()
    gui('open_file')
  end,
  open_folder = function()
    gui('open_folder')
  end,
  save = function()
    in_editor(function()
      if vim.api.nvim_buf_get_name(0) == '' then
        gui('save_as')
      else
        cmd('write')()
      end
    end)
  end,
  save_as = function()
    gui('save_as')
  end,
  save_all = cmd('wall'),
  save_session = function()
    Eshvi.session.prompt_save()
  end,
  close = close_editor,
  -- Edit
  undo = cmd('silent undo'),
  redo = cmd('silent redo'),
  cut = function()
    if visual() then
      feed('"+d')
    end
  end,
  copy = function()
    if visual() then
      feed('"+y')
    end
  end,
  paste = function()
    vim.paste(vim.fn.getreg('+', 1, true), -1)
  end,
  select_all = function()
    to_normal()
    feed('ggVG')
  end,
  find = function()
    to_normal()
    feed('/')
  end,
  find_next = cmd('normal! n'),
  find_prev = cmd('normal! N'),
  -- View
  command_line = function()
    to_normal()
    feed(':')
  end,
  command_palette = function()
    Eshvi.picker.palette()
  end,
  explorer = cmd('EshviExplorer toggle'),
  explorer_view = cmd('EshviExplorer focus'),
  source_control = cmd('EshviGit focus'),
  outline = cmd('EshviOutline focus'),
  panel = cmd('EshviPanel toggle'),
  problems = cmd('EshviProblems toggle'),
  git_output = cmd('EshviGitOutput toggle'),
  terminal = cmd('EshviTerminal toggle'),
  agent = cmd('EshviAgent toggle'),
  -- The file, or the selected lines, to the agent as context.
  send_to_agent = function()
    if not (Eshvi.ide and Eshvi.ide.send) then
      return
    end
    local buf = vim.api.nvim_get_current_buf()
    local path = vim.api.nvim_buf_get_name(buf)
    if vim.bo[buf].buftype ~= '' or path == '' then
      return vim.notify('Send to Agent: not a file', vim.log.levels.WARN)
    end
    if visual() then
      local l1, l2 = vim.fn.line('v'), vim.fn.line('.')
      to_normal()
      Eshvi.ide.send(path, math.min(l1, l2), math.max(l1, l2))
    else
      Eshvi.ide.send(path)
    end
  end,
  preview = function()
    vim.cmd('EshviPreview')
  end,
  preview_side = function()
    vim.cmd('EshviPreview side')
  end,
  zoom_in = function()
    gui('zoom_in')
  end,
  zoom_out = function()
    gui('zoom_out')
  end,
  zoom_reset = function()
    gui('zoom_reset')
  end,
  -- Go
  back = function()
    to_normal()
    feed('<C-o>')
  end,
  forward = function()
    to_normal()
    feed('<C-i>')
  end,
  definition = function()
    vim.lsp.buf.definition()
  end,
  references = function()
    vim.lsp.buf.references()
  end,
  goto_file = function()
    Eshvi.picker.smart()
  end,
  symbols = function()
    vim.lsp.buf.document_symbol()
  end,
  next_problem = function()
    vim.diagnostic.jump({ count = 1 })
  end,
  prev_problem = function()
    vim.diagnostic.jump({ count = -1 })
  end,
  -- Terminal
  new_terminal = cmd('EshviTerminal new'),
  next_terminal = cmd('EshviTerminal next'),
  prev_terminal = cmd('EshviTerminal prev'),
  close_terminal = cmd('EshviTerminal close'),
  -- Window
  split_right = function()
    in_editor(cmd('vsplit'))
  end,
  split_down = function()
    in_editor(cmd('split'))
  end,
  close_split = cmd('close'),
  -- Help
  help = cmd('help'),
  tutor = cmd('Tutor'),
}

function Eshvi.menu(name)
  local action = actions[name]
  if action then
    action()
  end
end

function Eshvi.save_as(path)
  in_editor(cmd('saveas ' .. vim.fn.fnameescape(path)))
end

-- { menu, priority, items }; an item is { name, shortcut, action, needs }
-- (`needs` an option: the terminal panel) or '-' (a separator).
local terminal = opts.terminal_enabled
local agent = opts.agent_enabled
local menus = {
  { 'File', 10, {
    { 'New File', '⌘N', 'new_file' },
    { 'Open…', '⌘O', 'open_file' },
    { 'Open Folder…', '', 'open_folder' },
    '-',
    { 'Save', '⌘S', 'save' },
    { 'Save As…', '⇧⌘S', 'save_as' },
    { 'Save All', '⌥⌘S', 'save_all' },
    '-',
    { 'Save Session…', '', 'save_session' },
    '-',
    { 'Close Editor', '⌘W', 'close' },
  } },
  { 'Edit', 20, {
    { 'Undo', '⌘Z', 'undo' },
    { 'Redo', '⇧⌘Z', 'redo' },
    '-',
    { 'Cut', '⌘X', 'cut' },
    { 'Copy', '⌘C', 'copy' },
    { 'Paste', '⌘V', 'paste' },
    { 'Select All', '⌘A', 'select_all' },
    '-',
    { 'Find', '⌘F', 'find' },
    { 'Find Next', '⌘G', 'find_next' },
    { 'Find Previous', '⇧⌘G', 'find_prev' },
  } },
  { 'View', 30, {
    -- (The native picker has the palette on ⇧⌘P; without it the command line keeps the key.)
    { 'Command Palette…', '⇧⌘P', 'command_palette', Eshvi.picker ~= nil },
    { 'Command Line', Eshvi.picker ~= nil and '' or '⇧⌘P', 'command_line' },
    '-',
    { 'Side Bar', '⌘B', 'explorer' },
    { 'Explorer', '⇧⌘E', 'explorer_view' },
    { 'Source Control', '⌃⇧G', 'source_control' },
    { 'Outline', '', 'outline' },
    { 'Panel', '⌘J', 'panel', terminal },
    { 'Problems', '⇧⌘M', 'problems', terminal },
    { 'Git Output', '⇧⌘U', 'git_output', terminal },
    { 'Terminal', '⌃`', 'terminal', terminal },
    { 'Agent', '⌥⌘B', 'agent', agent },
    { 'Send to Agent', '⌥⌘K', 'send_to_agent', agent },
    '-',
    { 'Markdown Preview', '⇧⌘V', 'preview' },
    { 'Markdown Preview to the Side', '⌥⇧⌘V', 'preview_side' },
    '-',
    { 'Zoom In', '⌘=', 'zoom_in' },
    { 'Zoom Out', '⌘-', 'zoom_out' },
    { 'Actual Size', '⌘0', 'zoom_reset' },
  } },
  { 'Go', 40, {
    { 'Back', '⌃-', 'back' },
    { 'Forward', '⌃⇧-', 'forward' },
    '-',
    { 'Go to File…', '⌘P', 'goto_file', Eshvi.picker ~= nil },
    { 'Go to Definition', 'F12', 'definition' },
    { 'Go to References', '⇧F12', 'references' },
    { 'Go to Symbol in File…', '⇧⌘O', 'symbols' },
    '-',
    { 'Next Problem', 'F8', 'next_problem' },
    { 'Previous Problem', '⇧F8', 'prev_problem' },
  } },
  terminal and { 'Terminal', 50, {
    { 'New Terminal', '⌃⇧`', 'new_terminal' },
    { 'Next Terminal', '', 'next_terminal' },
    { 'Previous Terminal', '', 'prev_terminal' },
    '-',
    { 'Close Terminal', '', 'close_terminal' },
  } } or nil,
  { 'Window', 60, {
    { 'Split Right', '⌘\\', 'split_right' },
    { 'Split Down', '', 'split_down' },
    { 'Close Split', '', 'close_split' },
  } },
  { 'Help', 9999, {
    { 'Neovim Help', '', 'help' },
    { 'Vim Tutor', '', 'tutor' },
  } },
}

-- Menu names escape spaces and dots.
local function escape(name)
  return (name:gsub('([ .\\])', '\\%1'))
end

-- Our items' actions, and checkmarks, by path, for the GUI (an edit action
-- in a GUI text field goes to the field).
local action_of, checked_of = {}, {}

local function define(pri, path, rhs)
  for _, mode in ipairs({ 'noremenu', 'inoremenu', 'cnoremenu', 'tlnoremenu' }) do
    vim.cmd(('%s <silent> %s %s %s'):format(mode, pri, path, rhs))
  end
end

for _, menu in pairs(menus) do
  local top, prio, items = menu[1], menu[2], menu[3]
  for i, item in ipairs(items) do
    local pri = ('%d.%d'):format(prio, i * 10)
    if item == '-' then
      vim.cmd(('anoremenu %s %s.-sep%d- <Nop>'):format(pri, top, i))
    elseif item[4] ~= false then
      local name, key, action = item[1], item[2], item[3]
      local path = escape(top) .. '.' .. escape(name) .. (key ~= '' and '<Tab>' .. key:gsub('\\', '\\\\') or '')
      define(pri, path, ("<Cmd>lua Eshvi.menu('%s')<CR>"):format(action))
      action_of[top .. '\n' .. name] = action
    end
  end
end

-- Menus that follow the session: File › Open Recent and Buffers. Each is
-- defined again only when its items change.
local recent = {} -- files opened in this session, newest first
-- Those opened before the GUI attached (the command line's files).
local opened = vim.tbl_filter(function(b)
  return b.name ~= '' and vim.bo[b.bufnr].buftype == ''
end, vim.fn.getbufinfo({ buflisted = 1 }))
table.sort(opened, function(x, y)
  return x.lastused > y.lastused
end)
for _, b in ipairs(opened) do
  table.insert(recent, b.name)
end
local shown_recent, shown_buffers = {}, {}
local signature = {}

-- `items`: { name, checked } or '-'; item i runs `run(i)`.
local function set_items(path, pri, items, run, disabled)
  local sig = vim.inspect({ items, disabled })
  if signature[path] == sig then
    return false
  end
  signature[path] = sig
  for _, unmenu in ipairs({ 'aunmenu', 'iunmenu', 'cunmenu', 'tlunmenu' }) do
    pcall(vim.cmd, unmenu .. ' ' .. path)
  end
  local parent = path:match('([^.]+)$'):gsub('\\(.)', '%1')
  for i, item in ipairs(items) do
    local pri = ('%s.%d'):format(pri, i * 10)
    if item == '-' then
      vim.cmd(('anoremenu %s %s.-sep%d- <Nop>'):format(pri, path, i))
    else
      define(pri, path .. '.' .. escape(item[1]), ("<Cmd>lua Eshvi.%s(%d)<CR>"):format(run, i))
      checked_of[parent .. '\n' .. item[1]] = item[2]
      if disabled and disabled[i] then
        for _, mode in ipairs({ 'menu', 'imenu', 'cmenu', 'tlmenu' }) do
          vim.cmd(('%s disable %s.%s'):format(mode, path, escape(item[1])))
        end
      end
    end
  end
  return true
end

-- A name a menu item can have: `&`, `|` and tabs would be read as menu
-- syntax.
local function nameable(name)
  return not name:find('[&|\t\n]') and not name:find('<Tab>', 1, true)
end

local function update_recent()
  local cur = vim.api.nvim_buf_get_name(vim.api.nvim_win_get_buf(editor_win() or 0))
  local files, seen = {}, { [cur] = true }
  for _, list in ipairs({ recent, vim.v.oldfiles }) do
    for _, f in ipairs(list) do
      if #files == 10 then
        break
      end
      if not seen[f] and vim.fn.filereadable(f) == 1 and nameable(vim.fn.fnamemodify(f, ':~')) then
        seen[f] = true
        table.insert(files, f)
      end
    end
  end
  shown_recent = files
  local items = {}
  for _, f in ipairs(files) do
    table.insert(items, { vim.fn.fnamemodify(f, ':~') })
  end
  if #items > 0 then
    table.insert(items, '-')
  end
  table.insert(items, { 'Clear Recently Opened' })
  return set_items('File.Open\\ Recent', '10.35', items, 'menu_recent', { [#items] = #files == 0 })
end

-- File › Open Session and Delete Session: the named sessions, newest first.
local shown_sessions = {}
local function update_sessions()
  local names, items = {}, {}
  for _, s in ipairs(Eshvi.session and Eshvi.session.list() or {}) do
    if nameable(s.name) then
      table.insert(names, s.name)
      table.insert(items, { s.name })
    end
  end
  shown_sessions = names
  if #items == 0 then
    items = { { 'No Sessions' } }
  end
  local disabled = { [1] = #names == 0 }
  local opened = set_items('File.Open\\ Session', '10.95', items, 'menu_open_session', disabled)
  local deleted = set_items('File.Delete\\ Session', '10.96', items, 'menu_delete_session', disabled)
  return opened or deleted
end

local function update_buffers()
  local win = editor_win()
  local cur = win and vim.api.nvim_win_get_buf(win)
  local items, bufs = {}, {}
  for _, b in ipairs(vim.fn.getbufinfo({ buflisted = 1 })) do
    if vim.bo[b.bufnr].buftype == '' then
      local name = b.name ~= '' and vim.fn.fnamemodify(b.name, ':~:.') or ('[No Name] ' .. b.bufnr)
      if nameable(name) then
        table.insert(items, { name, b.bufnr == cur })
        table.insert(bufs, b.bufnr)
      end
    end
  end
  shown_buffers = bufs
  if #items == 0 then
    items = { { '[No Name]' } }
  end
  return set_items('Buffers', '45', items, 'menu_buffer', { [1] = #bufs == 0 })
end

function Eshvi.menu_recent(i)
  local f = shown_recent[i]
  if f then
    in_editor(cmd('drop ' .. vim.fn.fnameescape(f)))
  else
    recent = {}
    vim.v.oldfiles = {}
    Eshvi.menu_update()
  end
end

function Eshvi.menu_open_session(i)
  if shown_sessions[i] then
    Eshvi.session.load(shown_sessions[i])
  end
end

function Eshvi.menu_delete_session(i)
  local name = shown_sessions[i]
  if name then
    vim.ui.select({ 'Delete', 'Cancel' }, { prompt = 'Delete session "' .. name .. '"?' }, function(choice)
      if choice == 'Delete' then
        Eshvi.session.delete(name)
      end
    end)
  end
end

function Eshvi.menu_buffer(i)
  local b = shown_buffers[i]
  if b and vim.api.nvim_buf_is_valid(b) then
    in_editor(cmd('buffer ' .. b))
  end
end

-- A shortcut's keys in Neovim's notation, as the GUI sends them (⇧⌘S is
-- <D-S>, ⌃⇧- is <C-_>).
local shifted = {
  ['`'] = '~', ['-'] = '_', ['='] = '+', ['['] = '{', [']'] = '}', ['\\'] = '|', [';'] = ':', ["'"] = '"',
  [','] = '<', ['.'] = '>', ['/'] = '?', ['0'] = ')', ['1'] = '!', ['2'] = '@', ['3'] = '#', ['4'] = '$',
  ['5'] = '%', ['6'] = '^', ['7'] = '&', ['8'] = '*', ['9'] = '(',
}
local key_names = { ['\\'] = 'Bslash', ['|'] = 'Bar', ['<'] = 'lt', [' '] = 'Space' }
local function keys_of(shortcut)
  local m, rest = {}, shortcut
  while true do
    local sym = rest:match('^[\226][\128-\191][\128-\191]')
    local mod = sym and ({ ['⌃'] = 'C', ['⌥'] = 'M', ['⇧'] = 'S', ['⌘'] = 'D' })[sym]
    if not mod then
      break
    end
    m[mod] = true
    rest = rest:sub(#sym + 1)
  end
  if rest:match('^F%d+$') then
    return ('<%s%s%s%s%s>'):format(m.C and 'C-' or '', m.S and 'S-' or '', m.M and 'M-' or '', m.D and 'D-' or '', rest)
  elseif #rest ~= 1 then
    return nil
  end
  local c = m.S and (rest:match('%a') and rest:upper() or shifted[rest] or rest) or rest:lower()
  if m.C and c:match('%a') then
    c = c:lower()
  end
  return ('<%s%s%s%s>'):format(m.C and 'C-' or '', m.M and 'M-' or '', m.D and 'D-' or '', key_names[c] or c)
end

-- A shortcut the user mapped in Neovim (a global Normal mode mapping)
-- goes to Neovim: the menu item loses it.
local function user_mapped(shortcut)
  local keys = shortcut ~= '' and keys_of(shortcut)
  local map = keys and vim.fn.maparg(keys, 'n', false, true)
  return map and next(map) ~= nil and map.buffer == 0
end

-- The items of Eshvi's own menus, for the command palette: { top, name, key,
-- action, path } (`path` as `:emenu` takes it), from the tables above so that
-- nothing is listed twice. A shortcut the user mapped in Neovim is not shown, as
-- the menu bar doesn't. Open Recent and Buffers follow the session and are not here.
function Eshvi.menu_items()
  local tops = {}
  for _, menu in pairs(menus) do
    table.insert(tops, menu)
  end
  table.sort(tops, function(a, b)
    return a[2] < b[2]
  end)
  local items = {}
  for _, menu in ipairs(tops) do
    for _, item in ipairs(menu[3]) do
      if item ~= '-' and item[4] ~= false then
        table.insert(items, {
          top = menu[1],
          name = item[1],
          key = user_mapped(item[2]) and '' or item[2],
          action = item[3],
          path = escape(menu[1]) .. '.' .. escape(item[1]),
        })
      end
    end
  end
  return items
end

-- An item is enabled unless `:menu disable` (Normal mode, else any mode).
local function enabled(m)
  local maps = m.mappings or {}
  local map = maps.n or maps.v or maps.i or select(2, next(maps))
  return not map or map.enabled ~= 0
end

-- The menu tree, for the GUI: every menu but Neovim's internal ones
-- (PopUp, ToolBar, ]Hidden).
local function convert(m, parent)
  local id = (parent or '') .. '\n' .. m.name
  local key = m.actext or ''
  local node = {
    name = m.name,
    key = user_mapped(key) and '' or key,
    action = action_of[id],
    enabled = enabled(m),
    checked = checked_of[id],
  }
  if m.submenus then
    node.children = {}
    for _, sub in ipairs(m.submenus) do
      if sub.hidden ~= 1 then
        table.insert(node.children, convert(sub, m.name))
      end
    end
  end
  return node
end

local function report()
  local top = {}
  for _, m in ipairs(vim.fn.menu_get('')) do
    if m.hidden ~= 1 and m.name ~= 'PopUp' and m.name ~= 'ToolBar' and not m.name:match('^%]') then
      table.insert(top, convert(m))
    end
  end
  vim.rpcnotify(chan, 'gui_menu', { menus = top })
end

-- Report the menus again when Open Recent or Buffers changed.
function Eshvi.menu_update()
  local changed = update_recent()
  if update_sessions() or update_buffers() or changed then
    report()
  end
end

local group = vim.api.nvim_create_augroup('eshvi_menu', { clear = true })
local pending = false
vim.api.nvim_create_autocmd({ 'BufEnter', 'BufAdd', 'BufDelete', 'BufWipeout', 'BufFilePost' }, {
  group = group,
  callback = function(a)
    if a.event == 'BufEnter' and vim.bo[a.buf].buftype == '' and a.file ~= '' then
      local f = vim.fn.fnamemodify(a.file, ':p')
      for i, r in ipairs(recent) do
        if r == f then
          table.remove(recent, i)
          break
        end
      end
      table.insert(recent, 1, f)
    end
    if not pending and vim.v.vim_did_enter == 1 then
      pending = true
      vim.schedule(function()
        pending = false
        Eshvi.menu_update()
      end)
    end
  end,
})

-- :EshviMenu updates the menu bar after `:menu` changes (menus defined in
-- the user's config are picked up at startup).
vim.api.nvim_create_user_command('EshviMenu', function()
  update_recent()
  update_sessions()
  update_buffers()
  report()
end, { desc = 'eshvi: update the native menu bar' })
if vim.v.vim_did_enter == 1 then
  vim.schedule(vim.cmd.EshviMenu)
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = function() vim.cmd.EshviMenu() end })
end
