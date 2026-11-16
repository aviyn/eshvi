-- eshvi: sessions. Neovim's part is `:mksession` (tabs, splits, files, cursors, folds,
-- the working folder); what it can't hold goes in a JSON file beside it: the Markdown
-- previews, the editor windows' sizes and the GUI's part (side bar shown, width, view,
-- expanded folders, the bottom and agent panels). Older files have `agent` and `panel`
-- keys beside; they are ignored.
-- This module is the only writer of both files.
--   :EshviSession save [name] | load {name} | delete {name}   (`!`: no questions)
--   Eshvi.session.set_gui(state)   the GUI's part, pushed whenever it changes
--   gui_session { gui }            sent after a load, for the GUI to apply
-- Files: `<state>/eshvi/sessions/auto/<hash of the folder>.{vim,json}` (saved on quit,
-- restored at start) and `.../named/<name>.{vim,json}`.
local chan, opts = ...
opts = opts or {}

_G.Eshvi = _G.Eshvi or {}
local S = {}
Eshvi.session = S

-- No `options`/`localoptions` (a plugin's options aren't frozen), no `terminal`.
local SESSIONOPTIONS = 'buffers,curdir,folds,help,tabpages,winsize'

-- The GUI's part, as it last sent it.
local gui_state = nil
-- Neovim got files on its command line: no automatic session, either way.
local started_with_files = vim.fn.argc() > 0

local function notify(msg, level)
  vim.notify(msg, level or vim.log.levels.INFO)
end

-- Where the files go (`g:eshvi_session_dir` is for tests), like bookmarks.json:
-- `$XDG_STATE_HOME/eshvi/` (default `~/.local/state/eshvi/`).
local function root()
  local dir = vim.g.eshvi_session_dir
  if type(dir) == 'string' and dir ~= '' then
    return dir
  end
  local base = vim.env.XDG_STATE_HOME
  if not (base and base ~= '' and vim.fn.isabsolutepath(base) == 1) then
    local home = vim.env.HOME
    base = home and home ~= '' and (home .. '/.local/state') or vim.env.APPDATA
  end
  return base and (base .. '/eshvi/sessions') or nil
end

local function real(path)
  return vim.uv.fs_realpath(path) or path
end

local function cwd()
  return real(vim.fn.getcwd(-1, -1))
end

-- The folders that get no automatic session, and from where the latest one restores: `/`
-- and the home folder (where the Dock starts Neovim).
local function is_home_or_root(dir)
  local home = vim.env.HOME
  return dir == '/' or (home ~= nil and home ~= '' and dir == real(home))
end

-- Writing aside, then renaming over the real file.
local function write_atomic(path, text)
  vim.fn.mkdir(vim.fs.dirname(path), 'p')
  local tmp = path .. '.tmp'
  local f, err = io.open(tmp, 'wb')
  if not f then
    return nil, err
  end
  f:write(text)
  f:close()
  return os.rename(tmp, path)
end

local function read_file(path)
  local f = io.open(path, 'rb')
  if not f then
    return nil
  end
  local text = f:read('*a')
  f:close()
  return text
end

local function read_json(path)
  local text = read_file(path)
  if not text then
    return nil
  end
  local ok, data = pcall(vim.json.decode, text)
  return ok and type(data) == 'table' and data or nil
end

-- Where a session's two files are: `base` .. '.vim' and '.json'.
local function auto_base(dir)
  local r = root()
  return r and (r .. '/auto/' .. vim.fn.sha256(dir):sub(1, 16))
end

local function named_base(name)
  local r = root()
  return r and (r .. '/named/' .. name)
end

-- A name that is a file name: no folders, nothing hidden, nothing odd.
local function valid_name(name)
  name = vim.trim(name or '')
  if name == '' or #name > 120 or name:find('[/\\%c]') or name:sub(1, 1) == '.' then
    return nil
  end
  return name
end

local function default_name()
  local base = vim.fs.basename(cwd())
  return valid_name(base) or 'session'
end

-- The named sessions, newest first: { name, time }.
function S.list()
  local r = root()
  local found = {}
  local dir = r and vim.uv.fs_scandir(r .. '/named')
  while dir do
    local name, kind = vim.uv.fs_scandir_next(dir)
    if not name then
      break
    end
    local base = name:match('^(.+)%.vim$')
    if base and kind ~= 'directory' then
      local st = vim.uv.fs_stat(r .. '/named/' .. name)
      table.insert(found, { name = base, time = st and (st.mtime.sec + st.mtime.nsec / 1e9) or 0 })
    end
  end
  table.sort(found, function(a, b)
    if a.time ~= b.time then
      return a.time > b.time
    end
    return a.name < b.name
  end)
  return found
end

-- Non-floating windows of a tab page, in window number order.
local function tab_wins(tab)
  return vim.tbl_filter(function(w)
    return vim.api.nvim_win_get_config(w).relative == ''
  end, vim.api.nvim_tabpage_list_wins(tab))
end

-- Run an Ex command, saying what went wrong.
local function run(cmd)
  local ok, err = pcall(vim.cmd, cmd)
  if not ok then
    notify(tostring(err):gsub('^Vim%(%w+%):', ''), vim.log.levels.ERROR)
  end
  return ok
end

-- The windows' sizes, by tab and window number, with the screen's: { lines, columns,
-- tabs = { { { w, h }, ... }, ... } }.
local function geometry()
  local tabs = {}
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    local sizes = {}
    for _, w in ipairs(tab_wins(tab)) do
      table.insert(sizes, { w = vim.api.nvim_win_get_width(w), h = vim.api.nvim_win_get_height(w) })
    end
    table.insert(tabs, sizes)
  end
  return { lines = vim.o.lines, columns = vim.o.columns, tabs = tabs }
end

-- Put the sizes back, in tabs that have as many windows as then, on a
-- screen of the same size (else the layout stays as `:mksession`'s proportions made it).
-- In window order, twice: setting a window moves its neighbour's edge. Setting windows'
-- heights can leave rows to the command line: it is given back.
local function apply_geometry(geo)
  if type(geo) ~= 'table' or geo.lines ~= vim.o.lines or geo.columns ~= vim.o.columns then
    return false
  end
  local cmdheight = vim.o.cmdheight
  for ti, tab in ipairs(vim.api.nvim_list_tabpages()) do
    local sizes, wins = type(geo.tabs) == 'table' and geo.tabs[ti], tab_wins(tab)
    if type(sizes) == 'table' and #sizes == #wins then
      for _ = 1, 2 do
        for n, w in ipairs(wins) do
          local size = sizes[n]
          if tonumber(size.w) then
            pcall(vim.api.nvim_win_set_width, w, math.floor(size.w))
          end
          if tonumber(size.h) then
            pcall(vim.api.nvim_win_set_height, w, math.floor(size.h))
          end
        end
      end
    end
  end
  if vim.o.cmdheight ~= cmdheight then
    vim.o.cmdheight = cmdheight
  end
  return true
end

-- A session loaded while the screen was another size (at start the GUI's window may
-- still be settling): its sizes are put back when the screen reaches the size they were
-- saved at, if that is soon.
local waiting = 0
local function apply_geometry_soon(geo)
  if apply_geometry(geo) or type(geo) ~= 'table' then
    return
  end
  waiting = waiting + 1
  local mine = waiting
  vim.api.nvim_create_autocmd('VimResized', {
    group = vim.api.nvim_create_augroup('eshvi_session_geometry', { clear = true }),
    callback = function()
      if mine ~= waiting then
        return true
      end
      if apply_geometry(geo) then
        waiting = waiting + 1
        return true
      end
    end,
  })
  vim.defer_fn(function()
    waiting = waiting + 1
  end, 3000)
end

-- Markdown previews, from { tab, win, side } (the tab's and window's numbers): in place
-- first, then the sides, the last window in a tab first (a new window shifts the numbers
-- after it).
local function restore_previews(list)
  local tabs = vim.api.nvim_list_tabpages()
  list = vim.deepcopy(list)
  table.sort(list, function(a, b)
    if a.side ~= b.side then
      return not a.side
    end
    return a.tab == b.tab and a.win > b.win or a.tab < b.tab
  end)
  for _, p in ipairs(list) do
    local tab = tabs[p.tab]
    local win = tab and tab_wins(tab)[p.win]
    if win and vim.bo[vim.api.nvim_win_get_buf(win)].filetype == 'markdown' then
      vim.api.nvim_win_call(win, function()
        pcall(vim.cmd, p.side and 'EshviPreview side' or 'EshviPreview')
      end)
    end
  end
end

-- Everything that goes in the session, as { vim = the `.vim` text, data = the JSON's
-- table }. The side previews' scratch windows are closed for the `:mksession` call (its
-- file has none) and put back before this returns when `restore`; nothing redraws in
-- between. (The panels are the GUI's, not windows: the GUI's part of the session holds them.)
local function capture(restore)
  local tabs = vim.api.nvim_list_tabpages()
  local cur_tab, cur_win, mode = vim.api.nvim_get_current_tabpage(), vim.api.nvim_get_current_win(), vim.fn.mode()
  local geo = geometry()

  local sides = {}
  for ti, tab in ipairs(tabs) do
    for _, w in ipairs(tab_wins(tab)) do
      if vim.w[w].eshvi_preview_of then
        table.insert(sides, { tab = ti, buf = vim.w[w].eshvi_preview_of, win = w })
      end
    end
  end
  for _, s in ipairs(sides) do
    pcall(vim.api.nvim_win_close, s.win, true)
  end

  -- The previews, by tab and window number (counted without the windows closed above).
  local previews, rebuild = {}, {}
  for ti, tab in ipairs(tabs) do
    local wins = tab_wins(tab)
    for n, w in ipairs(wins) do
      if vim.w[w].eshvi_preview then
        table.insert(previews, { tab = ti, win = n, side = false })
      end
    end
    for _, s in ipairs(sides) do
      if s.tab == ti then
        for n, w in ipairs(wins) do
          if vim.api.nvim_win_get_buf(w) == s.buf then
            table.insert(previews, { tab = ti, win = n, side = true })
            table.insert(rebuild, previews[#previews])
            break
          end
        end
      end
    end
  end

  local tmp = vim.fn.tempname() .. '.vim'
  local old_opt, old_this = vim.o.sessionoptions, vim.v.this_session
  vim.o.sessionoptions = SESSIONOPTIONS
  local ok, err = pcall(vim.cmd, 'mksession! ' .. vim.fn.fnameescape(tmp))
  vim.o.sessionoptions = old_opt
  pcall(vim.api.nvim_set_vvar, 'this_session', old_this)
  local text = ok and read_file(tmp) or nil
  os.remove(tmp)

  if restore then
    local equal = vim.o.equalalways
    vim.o.equalalways = false
    restore_previews(rebuild)
    vim.o.equalalways = equal
    apply_geometry(geo)
    -- Back where the user was, in the mode they were in.
    if vim.api.nvim_tabpage_is_valid(cur_tab) then
      vim.api.nvim_set_current_tabpage(cur_tab)
    end
    if vim.api.nvim_win_is_valid(cur_win) then
      vim.api.nvim_set_current_win(cur_win)
    end
    if mode:sub(1, 1) == 't' and vim.bo.buftype == 'terminal' then
      vim.cmd('startinsert')
    elseif mode:sub(1, 1) ~= 'i' then
      vim.cmd('stopinsert')
    end
  end

  if not text then
    return nil, tostring(err or 'mksession wrote nothing')
  end
  return {
    vim = text,
    data = {
      version = 1,
      cwd = cwd(),
      saved = os.time(),
      gui = gui_state,
      previews = previews,
      geometry = geo,
    },
  }
end

-- Both files of a session, `.vim` first.
local function commit(base, cap, name)
  if not base then
    return nil, 'no folder for sessions (HOME is not set)'
  end
  cap.data.name = name
  local ok, err = write_atomic(base .. '.vim', cap.vim)
  if not ok then
    return nil, err
  end
  ok, err = write_atomic(base .. '.json', vim.json.encode(cap.data))
  if not ok then
    return nil, err
  end
  return true
end

-- Whether a file is open: an empty Neovim has no session worth keeping.
local function has_files()
  for _, b in ipairs(vim.fn.getbufinfo({ buflisted = 1 })) do
    if b.name ~= '' and vim.bo[b.bufnr].buftype == '' then
      return true
    end
  end
  return false
end

-- Save the folder's automatic session (not for `/` or the home folder).
local function save_auto(cap)
  local dir = cwd()
  if is_home_or_root(dir) then
    return true
  end
  return commit(auto_base(dir), cap)
end

-- The unsaved file buffers' names.
local function modified()
  local names = {}
  for _, b in ipairs(vim.fn.getbufinfo({ bufmodified = 1 })) do
    if vim.bo[b.bufnr].buftype == '' then
      table.insert(names, b.name ~= '' and vim.fn.fnamemodify(b.name, ':~:.') or '[No Name]')
    end
  end
  return names
end

-- Save a named session (and the folder's automatic one, so the two agree).
function S.save(name)
  name = valid_name(name or default_name())
  if not name then
    return notify('EshviSession: not a usable session name', vim.log.levels.ERROR)
  end
  local cap, err = capture(true)
  if not cap then
    return notify('EshviSession: not saved: ' .. err, vim.log.levels.ERROR)
  end
  local ok, werr = commit(named_base(name), cap, name)
  if not ok then
    return notify('EshviSession: not saved: ' .. tostring(werr), vim.log.levels.ERROR)
  end
  if opts.session_auto then
    save_auto(cap)
  end
  notify('Session "' .. name .. '" saved')
  if Eshvi.menu_update then
    Eshvi.menu_update()
  end
  return true
end

-- `vim.ui.input` for the name (the File menu).
function S.prompt_save()
  vim.ui.input({ prompt = 'Save session as: ', default = default_name() }, function(name)
    if name and vim.trim(name) ~= '' then
      S.save(name)
    end
  end)
end

-- Load the session whose files start with `base`. Refused while file buffers are modified
-- unless `discard`.
local function load(base, discard, what)
  local file = base .. '.vim'
  if vim.fn.filereadable(file) ~= 1 then
    return notify('EshviSession: no such session: ' .. what, vim.log.levels.ERROR)
  end
  local names = modified()
  if #names > 0 and not discard then
    return notify(
      'EshviSession: not loaded, these buffers are modified: ' .. table.concat(names, ', ') .. ' (:EshviSession! load discards them)',
      vim.log.levels.WARN
    )
  end
  local data = read_json(base .. '.json') or {}

  -- The buffers wiped (the session's `only` closes the windows).
  for _, w in ipairs(vim.api.nvim_list_wins()) do
    vim.w[w].eshvi_preview = nil
  end
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_valid(b) and (vim.bo[b].buftype == '' or vim.bo[b].buftype == 'help') then
      pcall(vim.api.nvim_buf_delete, b, { force = true })
    end
  end
  local ok = run('silent source ' .. vim.fn.fnameescape(file))

  -- Previews, then the sizes (no 'equalalways' while windows are added), then the keyboard
  -- where the session had it.
  local cur_tab, cur_win = vim.api.nvim_get_current_tabpage(), vim.api.nvim_get_current_win()
  local equal = vim.o.equalalways
  vim.o.equalalways = false
  restore_previews(type(data.previews) == 'table' and data.previews or {})
  vim.o.equalalways = equal
  apply_geometry_soon(data.geometry)
  if vim.api.nvim_tabpage_is_valid(cur_tab) then
    vim.api.nvim_set_current_tabpage(cur_tab)
  end
  if vim.api.nvim_win_is_valid(cur_win) then
    vim.api.nvim_set_current_win(cur_win)
  end
  vim.cmd('stopinsert')

  if type(data.gui) == 'table' then
    gui_state = data.gui
    vim.rpcnotify(chan, 'gui_session', { gui = data.gui })
  end
  if ok and Eshvi.menu_update then
    Eshvi.menu_update()
  end
  return ok
end

function S.load(name, discard)
  name = valid_name(name)
  if not name then
    return notify('EshviSession: not a usable session name', vim.log.levels.ERROR)
  end
  local base = named_base(name)
  if base and load(base, discard, name) then
    notify('Session "' .. name .. '" loaded')
  end
end

function S.delete(name)
  name = valid_name(name)
  local base = name and named_base(name)
  if not base or vim.fn.filereadable(base .. '.vim') ~= 1 then
    return notify('EshviSession: no such session: ' .. tostring(name), vim.log.levels.ERROR)
  end
  os.remove(base .. '.vim')
  os.remove(base .. '.json')
  notify('Session "' .. name .. '" deleted')
  if Eshvi.menu_update then
    Eshvi.menu_update()
  end
end

-- The GUI's part (side bar shown, width, view, expanded folders), as it is now.
function S.set_gui(state)
  gui_state = type(state) == 'table' and state or nil
end

-- The latest automatic session of a folder that is still there.
local function latest_auto()
  local r = root()
  local dir = r and vim.uv.fs_scandir(r .. '/auto')
  local best, best_time
  while dir do
    local name = vim.uv.fs_scandir_next(dir)
    if not name then
      break
    end
    local base = name:match('^(.+)%.json$')
    local data = base and read_json(r .. '/auto/' .. name)
    if data and type(data.cwd) == 'string' and vim.fn.isdirectory(data.cwd) == 1 then
      local time = tonumber(data.saved) or 0
      if not best_time or time > best_time then
        best, best_time = r .. '/auto/' .. base, time
      end
    end
  end
  return best
end

-- At start, with no files on the command line: the folder's automatic session; from `/`
-- or the home folder (the Dock), the latest one.
local function restore_auto()
  if started_with_files or vim.fn.argc() > 0 then
    return
  end
  local dir = cwd()
  local base
  if is_home_or_root(dir) then
    base = latest_auto()
  else
    base = auto_base(dir)
  end
  if base and vim.fn.filereadable(base .. '.vim') == 1 then
    load(base, true, 'automatic')
  end
end

-- Saved on quit, for the working folder.
local function save_on_quit()
  if started_with_files or not has_files() or is_home_or_root(cwd()) then
    return
  end
  local cap = capture(false)
  if cap then
    save_auto(cap)
  end
end

-- Called when everything is loaded (`[session] auto`).
function S.start()
  if not opts.session_auto then
    return
  end
  vim.api.nvim_create_autocmd('VimLeavePre', {
    group = vim.api.nvim_create_augroup('eshvi_session', { clear = true }),
    callback = function()
      pcall(save_on_quit)
    end,
  })
  if vim.v.vim_did_enter == 1 then
    vim.schedule(restore_auto)
  else
    vim.api.nvim_create_autocmd('VimEnter', {
      once = true,
      callback = function()
        vim.schedule(restore_auto)
      end,
    })
  end
end

local subcommands = { 'save', 'load', 'delete' }
vim.api.nvim_create_user_command('EshviSession', function(o)
  local sub, name = o.fargs[1], vim.trim(o.args:sub(#(o.fargs[1] or '') + 1))
  if sub == 'save' then
    S.save(name ~= '' and name or nil)
  elseif sub == 'load' then
    S.load(name, o.bang)
  elseif sub == 'delete' then
    S.delete(name)
  else
    notify('EshviSession: save [name], load {name} or delete {name}', vim.log.levels.ERROR)
  end
end, {
  nargs = '+',
  bang = true,
  complete = function(_, line)
    local words = vim.split(line, '%s+', { trimempty = false })
    if #words <= 2 then
      return subcommands
    end
    return vim.tbl_map(function(s)
      return s.name
    end, S.list())
  end,
  desc = 'eshvi: save, load or delete a named session',
})
