-- The native picker's Neovim side (`[neovim] config = "own"`). The GUI draws
-- the box, filters and ranks (`picker.rs`); this file knows what Neovim
-- knows (buffers, recent files, the items of a `vim.ui.select`) and does what
-- is accepted.
--
--   Eshvi.picker.files({ cwd, title })   project files (the GUI walks the folder)
--   Eshvi.picker.git_files()             tracked files (`git ls-files`, in the GUI)
--   Eshvi.picker.smart()                 buffers, recent files, then project files
--   Eshvi.picker.buffers()
--   Eshvi.picker.recent()
--   Eshvi.picker.pick({ title, items = { { text, desc, path, lnum, col } }, on_choice })
--     or live: Eshvi.picker.pick({ title, hint, ask = function(query, reply) end, on_choice })
--   Eshvi.picker.grep({ cwd, query, extensions })
--                                        live grep (the GUI searches, in process), with a preview;
--                                        `extensions = { 'sv', 'v' }` keeps it to those files
--   Eshvi.picker.grep_word({ extensions })
--                                        grep for the word under the cursor or the Visual selection, literal
--   Eshvi.picker.lines()                 the current buffer's lines, fuzzy, unsaved text included
--   Eshvi.picker.grep_buffers()          grep in the listed buffers' files
--   Eshvi.picker.lsp_definitions()       also lsp_declarations, lsp_references, lsp_implementations,
--                                        lsp_type_definitions: one answer is a jump, more a picker
--   Eshvi.picker.locations({ title, locations = { { uri, range } }, encoding })
--                                        LSP locations a plugin already has, in that same picker; false if none opened
--   Eshvi.picker.lsp_symbols()           the buffer's symbols; lsp_workspace_symbols() asks per query
--   Eshvi.picker.diagnostics({ buf })    all files', or the buffer's
--   Eshvi.picker.qflist()                the quickfix list; loclist() the window's location list
--   Eshvi.picker.git_log_file()          the file's history (`git log --follow`); git_log_line() the cursor line's
--
-- To the GUI: `gui_picker_open { id, title, source, cwd, files, multi, items,
-- preview, query, literal, grep_files, extensions, selected, numbered, live, hint }`, `gui_picker_close { id }`,
-- `gui_picker_items { id, gen, items }` (a live picker's rows for a query),
-- `gui_picker_preview { req, path, buf, first_line, lines, spans, lnum, total,
-- note }`. Back, as calls: `Eshvi.picker.accept(id, chosen, how)`,
-- `Eshvi.picker.cancel(id)`, `Eshvi.picker.preview(req, path_or_buf, lnum)` and
-- `Eshvi.picker.query(id, gen, text)`. `vim.ui.select` is replaced on VimEnter
-- (after the config, which plugins set it in); opt out with
-- `vim.g.eshvi_native_picker = false`.
local chan = ...

_G.Eshvi = _G.Eshvi or {}
local P = {}
Eshvi.picker = P

-- The open picker: { id, title, items, on_choice, done }. One at a time.
local current = nil
local last_id = 0

-- Files entered in this session, newest first (`v:oldfiles` only has the
-- ones from earlier sessions).
local session = {}

local function real(path)
  return vim.uv.fs_realpath(path) or path
end

-- The folder pickers work in: the explorer's root (the workspace, which `:cd`
-- changes but 'autochdir' following the file doesn't), else the working
-- directory. With 'autochdir' on (the own config has it) `getcwd()` is the
-- current file's folder, which would list only that.
local function project()
  local root = vim.g.eshvi_root
  return (type(root) == 'string' and root ~= '') and root or vim.fn.getcwd()
end

local function enabled()
  if vim.g.eshvi_native_picker == false then
    vim.notify('The native picker is off (vim.g.eshvi_native_picker = false)', vim.log.levels.WARN)
    return false
  end
  return true
end

-- A path as the picker shows it: relative to `cwd` when under it (comparing
-- real paths: `/var` is `/private/var`), else with `~`. And the path to open:
-- under `cwd` as `cwd` spells it, so the GUI's walk of the folder finds the
-- same one.
local function describe(path, cwd)
  local base = (real(cwd):gsub('/+$', '')) .. '/'
  local r = real(path)
  if r:sub(1, #base) == base then
    local rel = r:sub(#base + 1)
    return rel, (cwd:gsub('/+$', '')) .. '/' .. rel
  end
  return vim.fn.fnamemodify(r, ':~'), r
end

local function under(path, cwd)
  local base = (real(cwd):gsub('/+$', '')) .. '/'
  return real(path):sub(1, #base) == base
end

-- Tell a callback what was chosen, once. Errors in it are the user's.
local function finish(cur, item, idx)
  if cur.done then
    return
  end
  cur.done = true
  if item == nil and cur.on_cancel then
    -- (Nothing chosen: a live preview, a colorscheme tried on, is undone.)
    local ok, err = pcall(cur.on_cancel)
    if not ok then
      vim.notify(tostring(err), vim.log.levels.ERROR)
    end
  end
  if cur.on_choice then
    local ok, err = pcall(cur.on_choice, item, idx)
    if not ok then
      vim.notify(tostring(err), vim.log.levels.ERROR)
    end
  end
end

-- ---- Resume ----
--
-- The last picker that can be reopened: how (a call made again, so the rows
-- are read again, or the spec as it was, for the sources that depend on where
-- the cursor was) and what it held when it closed. A select, anything with an
-- `on_choice`, and the colorschemes are not resumable: the one before is.
local last = nil
-- The call being made / the state it is made with, taken by the `open` it reaches.
local building, resume_state = nil, nil

-- Make `f` a source that `resume` can call again.
local function resumable(f)
  local function call(...)
    local args = { n = select('#', ...), ... }
    building = {
      run = function()
        return f(unpack(args, 1, args.n))
      end,
    }
    local ok, err = pcall(f, ...)
    building = nil
    if not ok then
      error(err, 0)
    end
  end
  return call
end

-- ---- Opening ----

-- Wait for the app to say the box is up, then hand it what was typed meanwhile.
-- The app learns of the picker a round trip after Neovim has run the mapping; keys
-- typed in between are Neovim's typeahead, and would run as commands (a stray `c`
-- left an operator pending, and the next keys edited the buffer). They are taken
-- out of the typeahead and given to the field. `vim.wait` lets the RPC through.
local WAIT_MS = 100
local function take_typeahead(spec)
  -- (`g:eshvi_picker_wait_ms` is for tests: a longer wait tells waiting from not on a slow machine.)
  pcall(vim.wait, tonumber(vim.g.eshvi_picker_wait_ms) or WAIT_MS, function()
    return spec.opened == true
  end, 2)
  if current ~= spec then
    return
  end
  local keys = {}
  pcall(function()
    for _ = 1, 10000 do
      if vim.fn.getchar(1) == 0 then
        break
      end
      local ch = vim.fn.getcharstr(0)
      if ch == '' then
        break
      end
      table.insert(keys, vim.fn.keytrans(ch))
    end
  end)
  -- (Always, even with nothing: the box holds what is typed after this until it hears.)
  vim.rpcnotify(chan, 'gui_picker_typed', { id = spec.id, keys = keys })
end

local function open(spec)
  -- (The call this open is part of, taken before it may be scheduled.)
  local resume = spec.resume or building
  local state = resume_state
  if vim.in_fast_event() then
    return vim.schedule(function()
      spec.resume = resume
      resume_state = state
      open(spec)
      resume_state = nil
    end)
  end
  if spec.snapshot then
    resume = { snapshot = true }
  end
  if spec.on_choice or spec.no_resume then
    resume = nil
  end
  spec.resume = resume
  if state and resume then
    spec.query = state.query
    spec.want = state.row
  end
  resume_state = nil
  last_id = last_id + 1
  spec.id = last_id
  local old = current
  current = spec
  -- Another picker replaces this one, which is cancelled.
  if old then
    if old.resume then
      last = { resume = old.resume, spec = old }
    end
    finish(old, nil, nil)
    if current ~= spec then
      return
    end
  end
  vim.rpcnotify(chan, 'gui_picker_open', {
    id = spec.id,
    title = spec.title,
    source = spec.source,
    cwd = spec.cwd,
    files = spec.files,
    multi = spec.multi,
    items = spec.items,
    exclude = spec.exclude,
    preview = spec.preview,
    query = spec.query,
    literal = spec.literal,
    grep_files = spec.grep_files,
    extensions = spec.extensions,
    selected = spec.selected,
    numbered = spec.numbered,
    live = spec.live,
    hint = spec.hint,
    on_select = spec.on_select ~= nil,
    want = spec.want,
  })
  take_typeahead(spec)
end

-- The app has the box up and its field focused.
function P.opened(id)
  if current and current.id == id then
    current.opened = true
  end
end

-- The selection stayed on row `idx` of the list sent: a picker that applies it live.
function P.selected(id, idx)
  local cur = current
  if cur and cur.id == id and cur.on_select then
    local ok, err = pcall(cur.on_select, idx)
    if not ok then
      vim.notify(tostring(err), vim.log.levels.ERROR)
    end
  end
end

-- ---- What Neovim knows ----

local function buffer_items(cwd, skip_current)
  local items = {}
  local here = skip_current and vim.api.nvim_get_current_buf() or -1
  local infos = vim.fn.getbufinfo({ buflisted = 1 })
  table.sort(infos, function(a, b)
    if a.lastused ~= b.lastused then
      return a.lastused > b.lastused
    end
    return a.bufnr > b.bufnr
  end)
  for _, b in ipairs(infos) do
    if vim.bo[b.bufnr].buftype == '' and b.bufnr ~= here then
      if b.name ~= '' then
        local text, path = describe(b.name, cwd)
        table.insert(items, { text = text, path = path, buf = b.bufnr, group = 'buffer' })
      elseif b.changed == 1 then
        table.insert(items, { text = '[No Name]', desc = '#' .. b.bufnr, buf = b.bufnr, group = 'buffer' })
      end
    end
  end
  return items
end

local function recent_items(cwd, project_only, skip)
  local items, seen = {}, {}
  local function add(path)
    if path == '' or seen[path] then
      return
    end
    seen[path] = true
    if skip[path] or (project_only and not under(path, cwd)) or vim.fn.filereadable(path) ~= 1 then
      return
    end
    local text, full = describe(path, cwd)
    table.insert(items, { text = text, path = full, group = 'recent' })
  end
  for _, path in ipairs(session) do
    add(real(path))
  end
  for _, path in ipairs(vim.v.oldfiles or {}) do
    add(real(path))
  end
  return items
end

-- Real paths of the files in `items`.
local function paths_of(items)
  local set = {}
  for _, item in ipairs(items) do
    if item.path then
      set[real(item.path)] = true
    end
  end
  return set
end

local function current_file()
  local buf = vim.api.nvim_get_current_buf()
  if vim.bo[buf].buftype == '' and vim.api.nvim_buf_get_name(buf) ~= '' then
    return real(vim.api.nvim_buf_get_name(buf))
  end
end

-- ---- The sources ----

function P.files(opts)
  if not enabled() then
    return
  end
  opts = opts or {}
  open({
    title = opts.title or 'Find files',
    source = 'files',
    cwd = opts.cwd or project(),
    files = true,
    multi = true,
    items = {},
  })
end

function P.git_files()
  if not enabled() then
    return
  end
  open({ title = 'Git files', source = 'git_files', cwd = project(), files = true, multi = true, items = {} })
end

function P.buffers()
  if not enabled() then
    return
  end
  local cwd = project()
  open({ title = 'Buffers', source = 'items', cwd = cwd, files = true, multi = true, items = buffer_items(cwd, false) })
end

function P.recent()
  if not enabled() then
    return
  end
  local cwd = project()
  open({
    title = 'Recent files',
    source = 'items',
    cwd = cwd,
    files = true,
    multi = true,
    items = recent_items(cwd, false, {}),
  })
end

function P.smart()
  if not enabled() then
    return
  end
  local cwd = project()
  local items = buffer_items(cwd, true)
  local skip = paths_of(items)
  local here = current_file()
  local exclude
  if here then
    skip[here] = true
    local _, path = describe(here, cwd)
    exclude = path
  end
  vim.list_extend(items, recent_items(cwd, true, skip))
  open({ title = 'Find files', source = 'smart', cwd = cwd, files = true, multi = true, items = items, exclude = exclude })
end

-- Any list. `on_choice(item, idx)` gets the item as given; nothing chosen is
-- `on_choice(nil, nil)`. Without it, items with a `path` are opened.
--
-- With `ask = function(query, reply)` it is live, for a plugin whose list comes
-- from a server: each query asks, and `reply(items)` gives its rows, unfiltered
-- by the app. `hint` shows while the query is empty.
function P.pick(opts)
  if not enabled() then
    return
  end
  local function rows_of(given)
    local rows = {}
    for i, item in ipairs(given) do
      rows[i] = { text = item.text, desc = item.desc, path = item.path, lnum = item.lnum, col = item.col }
    end
    return rows
  end
  local given = opts.items or {}
  local spec = {
    title = opts.title or 'Select',
    source = 'items',
    cwd = opts.cwd or project(),
    files = false,
    multi = opts.on_choice == nil,
    items = rows_of(given),
    originals = given,
    on_choice = opts.on_choice,
  }
  if opts.ask then
    spec.live = true
    spec.hint = opts.hint
    spec.ask = function(text, reply)
      opts.ask(text, function(items)
        spec.originals = items
        reply(rows_of(items))
      end)
    end
  end
  open(spec)
end

-- Live grep: the GUI runs the search (ripgrep's libraries, in process) and
-- shows a preview beside the rows. `query` is what the field starts with.
function P.grep(opts)
  if not enabled() then
    return
  end
  opts = opts or {}
  open({
    title = opts.title or 'Grep',
    source = 'grep',
    cwd = opts.cwd or project(),
    files = false,
    multi = true,
    items = {},
    query = opts.query or '',
    literal = opts.literal or false,
    extensions = opts.extensions,
    preview = true,
    snapshot = true,
  })
end

-- The text selected in Visual mode (its first line), leaving Visual mode; else
-- the word under the cursor.
local function word_or_selection()
  local mode = vim.fn.mode()
  if mode:match('^[vV\22]') then
    local lines = vim.fn.getregion(vim.fn.getpos('v'), vim.fn.getpos('.'), { type = mode })
    vim.cmd('normal! \27')
    return lines[1] or ''
  end
  return vim.fn.expand('<cword>')
end

function P.grep_word(opts)
  if not enabled() then
    return
  end
  opts = opts or {}
  P.grep({ title = opts.title or 'Grep word', query = word_or_selection(), literal = true, extensions = opts.extensions })
end

-- Grep in the files of the listed buffers.
function P.grep_buffers()
  if not enabled() then
    return
  end
  local files = {}
  for _, b in ipairs(vim.fn.getbufinfo({ buflisted = 1 })) do
    if b.name ~= '' and vim.bo[b.bufnr].buftype == '' and vim.fn.filereadable(b.name) == 1 then
      table.insert(files, b.name)
    end
  end
  open({
    title = 'Grep buffers',
    source = 'grep',
    cwd = project(),
    files = false,
    multi = true,
    items = {},
    query = '',
    grep_files = files,
    preview = true,
    snapshot = true,
  })
end

-- The current buffer's lines, fuzzy: what is in the buffer, saved or not.
-- Blank lines are left out; the row at the cursor starts selected.
local MAX_LINES = 100000
function P.lines()
  if not enabled() then
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  local name = vim.api.nvim_buf_get_name(buf)
  local here = vim.api.nvim_win_get_cursor(0)[1]
  local path = (name ~= '' and vim.bo[buf].buftype == '') and name or nil
  local items, selected = {}, nil
  for i, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, MAX_LINES, false)) do
    local lead, text = line:match('^(%s*)(.-)%s*$')
    if text ~= '' then
      table.insert(items, { text = text, lnum = i, col = #lead + 1, buf = buf, path = path })
      if not selected and i >= here then
        selected = #items
      end
    end
  end
  open({
    title = 'Buffer lines',
    source = 'items',
    cwd = project(),
    files = false,
    multi = true,
    items = items,
    preview = true,
    numbered = true,
    selected = selected or #items,
  })
end

-- ---- What is accepted ----

local function usable(win)
  return vim.api.nvim_win_get_config(win).relative == '' and vim.bo[vim.api.nvim_win_get_buf(win)].buftype == ''
end

-- The window to open in: the current one if it holds a file, else (a list, a
-- plugin's side bar) the editor's.
local function enter_editor()
  if vim.fn.mode():match('^[iR]') then
    vim.cmd('stopinsert')
  end
  local editor = Eshvi.editor_window and Eshvi.editor_window()
  local moved = not usable(vim.api.nvim_get_current_win()) and editor
  if moved then
    vim.api.nvim_set_current_win(editor)
  end
end

local commands = { edit = 'drop', split = 'split', vsplit = 'vsplit', tab = 'tabedit' }

-- Put the cursor on a place in the file just opened. Without a jumplist entry
-- for where the file had it (`nvim_win_set_cursor` would add one, and Ctrl-O
-- would first go there): the jump is the one entry, made before it.
local function place(lnum, col)
  if lnum then
    pcall(vim.cmd, ('keepjumps call cursor(%d, %d)'):format(lnum, math.max(col or 1, 1)))
    vim.cmd('normal! zv')
  end
end

-- `before`, if given, runs in the window the file opens in, before it does
-- (the jumplist and the tag stack are that window's).
local function open_path(path, how, lnum, col, before)
  enter_editor()
  if before then
    before()
  end
  -- (A jump has made its one jumplist entry; `:drop` would add another, and
  -- Ctrl-O would stop in the new file.)
  local ok, err = pcall(vim.cmd, (before and 'keepjumps ' or '') .. (commands[how] or 'drop') .. ' ' .. vim.fn.fnameescape(path))
  if not ok then
    vim.notify((tostring(err):gsub('^Vim%(%w+%):', '')), vim.log.levels.ERROR)
    return
  end
  place(lnum, col)
end

local function open_buffer(buf, how, before)
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  enter_editor()
  if before then
    before()
  end
  local split = ({ split = 'sbuffer', vsplit = 'vertical sbuffer', tab = 'tab sbuffer' })[how]
  pcall(vim.cmd, (before and 'keepjumps ' or '') .. (split or 'buffer') .. ' ' .. buf)
end

function P.accept(id, chosen, how, state)
  local cur = current
  if not cur or cur.id ~= id then
    return
  end
  current = nil
  if cur.resume then
    last = { resume = cur.resume, spec = cur, state = state }
  end
  how = how or 'edit'
  local function item_of(c)
    return c.idx and cur.items[c.idx] or {}
  end
  if cur.on_choice then
    local idx = chosen[1] and chosen[1].idx
    local item = idx and (cur.originals or {})[idx]
    return finish(cur, item, idx)
  end
  cur.done = true
  if how == 'quickfix' then
    local qf = {}
    for _, c in ipairs(chosen) do
      local item = item_of(c)
      local path = c.path or item.path
      if path then
        table.insert(qf, {
          filename = path,
          lnum = c.lnum or item.lnum or 1,
          col = c.col or item.col or 1,
          text = c.text or item.desc or '',
        })
      end
    end
    vim.fn.setqflist({}, ' ', { title = cur.title, items = qf })
    return vim.cmd('copen')
  end
  -- A jump goes in the jumplist, and in the tag stack if it came from a
  -- request (so Ctrl-O and Ctrl-T return, as `vim.lsp.buf.definition`'s do).
  local before = cur.jump
    and function()
      pcall(vim.cmd, "normal! m'")
      if cur.jump.from then
        pcall(vim.fn.settagstack, vim.api.nvim_get_current_win(), {
          items = { { tagname = cur.jump.tag or '', from = cur.jump.from } },
        }, 't')
      end
    end
  for _, c in ipairs(chosen) do
    local item = item_of(c)
    local path = c.path or item.path
    if cur.accept then
      cur.accept(c, item, how)
    elseif path then
      open_path(path, how, c.lnum or item.lnum, c.col or item.col, before)
    elseif item.buf then
      open_buffer(item.buf, how, before)
      place(c.lnum or item.lnum, c.col or item.col)
    end
  end
end

function P.cancel(id, state)
  local cur = current
  if cur and cur.id == id then
    current = nil
    if cur.resume then
      last = { resume = cur.resume, spec = cur, state = state }
    end
    finish(cur, nil, nil)
  end
end

-- Neovim is going: nothing is left to answer.
function P.close()
  local cur = current
  if cur then
    current = nil
    vim.rpcnotify(chan, 'gui_picker_close', { id = cur.id })
    finish(cur, nil, nil)
  end
end

-- ---- vim.ui.select ----

function P.select(items, opts, on_choice)
  opts = opts or {}
  local format = opts.format_item or tostring
  local rows = {}
  for i, item in ipairs(items) do
    rows[i] = { text = (tostring(format(item)):gsub('%s*\n%s*', ' ')) }
  end
  local title = opts.prompt and (opts.prompt:gsub('%s*:?%s*$', '')) or ''
  P.pick({
    title = title ~= '' and title or 'Select',
    items = rows,
    on_choice = function(_, idx)
      on_choice(idx and items[idx], idx)
    end,
  })
end

local group = vim.api.nvim_create_augroup('eshvi_picker', { clear = true })

-- ---- The preview ----
--
-- What the GUI shows beside grep's rows: a window of lines around `lnum` with
-- the colors treesitter gives them. Nothing here opens a buffer or fires an
-- autocmd: the text comes from a loaded buffer (unsaved changes show) or is
-- read from the file, and is parsed as a string.

local PREVIEW_BYTES = 2 * 1024 * 1024
local PREVIEW_LINES = 200000
local PREVIEW_WINDOW = 1000

-- The colors of highlight groups, by name; reset with the colorscheme.
local hl_cache = {}
vim.api.nvim_create_autocmd('ColorScheme', {
  group = group,
  callback = function()
    hl_cache = {}
  end,
})

local function hl_style(name)
  local c = hl_cache[name]
  if c == nil then
    local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = name, link = false })
    c = false
    if ok and (hl.fg or hl.bold or hl.italic) then
      c = { fg = hl.fg or -1, bold = hl.bold and 1 or 0, italic = hl.italic and 1 or 0 }
    end
    hl_cache[name] = c
  end
  return c
end

-- The style of capture `@a.b` in `lang`: `@a.b.lang`, then `@a.b`, then `@a`
-- (the way Neovim falls back).
local function capture_style(capture, lang)
  local name = '@' .. capture .. '.' .. lang
  while #name > 1 do
    local style = hl_style(name)
    if style then
      return style
    end
    name = name:match('^(.*)%.[^.]*$') or ''
  end
end

-- Spans `{ line, start, end, fg, bold, italic }` (line: index into `lines`
-- from 0; columns are 0-based bytes), in the order to apply them: a later one
-- wins where they overlap, as Neovim's own do.
local function spans_of(lines, lang)
  local source = table.concat(lines, '\n')
  local parser = vim.treesitter.get_string_parser(source, lang)
  parser:parse(true)
  local found, seq, prioritized = {}, 0, false
  parser:for_each_tree(function(tree, ltree)
    local tree_lang = ltree:lang()
    local query = vim.treesitter.query.get(tree_lang, 'highlights')
    if not query then
      return
    end
    for id, node, metadata in query:iter_captures(tree:root(), source) do
      local capture = query.captures[id]
      local style = capture:sub(1, 1) ~= '_' and capture_style(capture, tree_lang)
      if style then
        local sr, sc, er, ec = node:range()
        local meta = metadata and (metadata[id] or metadata) or {}
        local priority = tonumber(meta.priority) or 100
        prioritized = prioritized or priority ~= 100
        for l = sr, er do
          local from = (l == sr) and sc or 0
          local to = (l == er) and ec or #(lines[l + 1] or '')
          if to > from then
            seq = seq + 1
            found[#found + 1] = { l, from, to, style.fg, style.bold, style.italic, priority, seq }
          end
        end
      end
    end
  end)
  if prioritized then
    table.sort(found, function(a, b)
      if a[7] ~= b[7] then
        return a[7] < b[7]
      end
      return a[8] < b[8]
    end)
  end
  for _, span in ipairs(found) do
    span[7], span[8] = nil, nil
  end
  return found
end

-- The parser's language for a file, if there is one.
local function language_of(path, buf)
  local ft = buf and vim.bo[buf].filetype or ''
  if ft == '' then
    ft = vim.filetype.match({ filename = path }) or ''
  end
  if ft == '' then
    return nil
  end
  local lang = vim.treesitter.language.get_lang(ft) or ft
  local ok, added = pcall(vim.treesitter.language.add, lang)
  if ok and added and vim.treesitter.query.get(lang, 'highlights') then
    return lang
  end
end

-- The last few files read, by path: their lines while the file is unchanged.
local files_seen = {}

-- A file's lines, or nil and what to say instead.
local function read_lines(path)
  local stat = vim.uv.fs_stat(path)
  if not stat or stat.type ~= 'file' then
    return nil, "Can't read this file"
  end
  if stat.size > PREVIEW_BYTES then
    return nil, 'Too large to preview'
  end
  local stamp = stat.mtime.sec .. '.' .. stat.mtime.nsec .. ':' .. stat.size
  for i, seen in ipairs(files_seen) do
    if seen.path == path and seen.stamp == stamp then
      table.remove(files_seen, i)
      table.insert(files_seen, 1, seen)
      return seen.lines
    end
  end
  local fd = vim.uv.fs_open(path, 'r', 438)
  if not fd then
    return nil, "Can't read this file"
  end
  local data = vim.uv.fs_read(fd, stat.size, 0)
  vim.uv.fs_close(fd)
  if not data then
    return nil, "Can't read this file"
  end
  if data:find('\0', 1, true) then
    return nil, 'Binary file'
  end
  local lines = vim.split(data, '\n', { plain = true })
  if lines[#lines] == '' then
    lines[#lines] = nil
  end
  if #lines > PREVIEW_LINES then
    return nil, 'Too large to preview'
  end
  table.insert(files_seen, 1, { path = path, stamp = stamp, lines = lines })
  files_seen[5] = nil
  return lines
end

-- A loaded buffer showing `path`, if any.
local function loaded_buffer(path)
  local want = real(path)
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and vim.bo[b].buftype == '' then
      local name = vim.api.nvim_buf_get_name(b)
      if name ~= '' and (name == path or real(name) == want) then
        return b
      end
    end
  end
end

local function preview_answer(target, lnum)
  local answer = { path = type(target) == 'string' and target or '', lnum = lnum, first_line = 1, lines = {}, spans = {}, total = 0, note = '' }
  local buf = type(target) == 'number' and target or nil
  if buf then
    if not vim.api.nvim_buf_is_valid(buf) or not vim.api.nvim_buf_is_loaded(buf) then
      answer.note = "Can't read this buffer"
      return answer
    end
    answer.buf = buf
    answer.path = vim.api.nvim_buf_get_name(buf)
  else
    buf = loaded_buffer(target)
  end
  local lines, total
  if buf then
    if vim.bo[buf].buftype ~= '' then
      answer.note = 'No preview'
      return answer
    end
    total = vim.api.nvim_buf_line_count(buf)
    if total > PREVIEW_LINES then
      answer.note = 'Too large to preview'
      return answer
    end
  else
    local all, note = read_lines(target)
    if not all then
      answer.note = note
      return answer
    end
    lines, total = all, #all
  end
  lnum = math.max(1, math.min(lnum or 1, math.max(total, 1)))
  local first = math.max(1, lnum - math.floor(PREVIEW_WINDOW / 2))
  local last = math.min(total, first + PREVIEW_WINDOW - 1)
  first = math.max(1, last - PREVIEW_WINDOW + 1)
  local window
  if buf then
    window = vim.api.nvim_buf_get_lines(buf, first - 1, last, false)
  else
    window = {}
    for i = first, last do
      window[#window + 1] = (lines[i]:gsub('\r$', ''))
    end
  end
  answer.first_line, answer.lines, answer.total, answer.lnum = first, window, total, lnum
  local lang = language_of(answer.path, buf)
  if lang then
    local ok, spans = pcall(spans_of, window, lang)
    if ok then
      answer.spans = spans
    end
  end
  return answer
end

-- `target` is a path or a buffer number. The answer is the `gui_picker_preview`
-- notification, for the request `req`; a failure is a plain preview, never an error.
function P.preview(req, target, lnum)
  local ok, answer = pcall(preview_answer, target, lnum)
  if not ok then
    answer = { path = type(target) == 'string' and target or '', lnum = lnum, first_line = 1, lines = {}, spans = {}, total = 0, note = "Can't read this file" }
  end
  answer.req = req
  vim.rpcnotify(chan, 'gui_picker_preview', answer)
end

-- ---- LSP locations, symbols, diagnostics, quickfix and location lists ----
--
-- None of these touches a file as a buffer to read its lines: a loaded
-- buffer's text (unsaved changes show) or the file read as the preview reads it.

local function lines_for(path)
  local b = loaded_buffer(path)
  if b then
    return vim.api.nvim_buf_get_lines(b, 0, -1, false)
  end
  return (read_lines(path))
end

-- The byte index (from 0) of `character` in `line`, counted in `encoding`'s
-- units (UTF-16 for most servers): a line with wide characters differs.
local function byte_of(line, character, encoding)
  if not line then
    return character
  end
  local ok, idx = pcall(vim.str_byteindex, line, encoding or 'utf-16', character, false)
  return (ok and idx) or #line
end

local SNIPPET_BYTES = 400

-- A line as a row shows it: indent and trailing space off, cut if long, with
-- the stretch `from`..`to` (bytes, from 0) in it, and in the whole line.
local function snippet(line, from, to)
  line = (line or ''):gsub('\r$', '')
  local lead = #line:match('^%s*')
  local text = (line:sub(lead + 1):gsub('%s+$', ''))
  if #text > SNIPPET_BYTES then
    text = text:sub(1, SNIPPET_BYTES + vim.str_utf_end(text, SNIPPET_BYTES))
  end
  to = math.max(to, from + 1)
  local a, b = math.max(from - lead, 0), math.min(to - lead, #text)
  return {
    text = text,
    ranges = b > a and { { a, b } } or {},
    line_ranges = { { from, math.min(to, math.max(#line, from + 1)) } },
  }
end

-- Sorted like the spec: the current file first, then by path and line.
local function sort_locations(rows)
  local here = current_file()
  local is_here = {}
  for _, row in ipairs(rows) do
    if is_here[row.path] == nil then
      is_here[row.path] = here ~= nil and real(row.path) == here
    end
  end
  table.sort(rows, function(a, b)
    if is_here[a.path] ~= is_here[b.path] then
      return is_here[a.path]
    end
    if a.text ~= b.text then
      return a.text < b.text
    end
    if a.lnum ~= b.lnum then
      return a.lnum < b.lnum
    end
    return a.col < b.col
  end)
end

-- `raw`: { uri, range, encoding } in the server's units. Rows with the
-- line in them, each place once.
local function location_rows(raw, cwd)
  local cache, seen, rows = {}, {}, {}
  local function lines(path)
    if cache[path] == nil then
      cache[path] = lines_for(path) or false
    end
    return cache[path] or nil
  end
  for _, r in ipairs(raw) do
    local path = vim.uri_to_fname(r.uri)
    local ls = lines(path)
    local s, e = r.range.start, r.range['end']
    local sline, eline = ls and ls[s.line + 1], ls and ls[e.line + 1]
    local c0 = byte_of(sline, s.character, r.encoding)
    local key = path .. ':' .. s.line .. ':' .. c0
    if not seen[key] then
      seen[key] = true
      local to = (e.line == s.line) and byte_of(eline, e.character, r.encoding) or #(sline or '')
      local rel, full = describe(path, cwd)
      local line = snippet(sline, c0, to)
      table.insert(rows, {
        text = rel,
        path = full,
        lnum = s.line + 1,
        col = c0 + 1,
        line = line,
        search = rel .. ' ' .. line.text,
        from0 = c0,
        to0 = to,
      })
    end
  end
  return rows
end

-- A Location, a LocationLink, one of them or a list: [{ uri, range }].
local function raw_locations(result, encoding, into)
  if not result then
    return
  end
  if result.uri or result.targetUri then
    result = { result }
  end
  for _, loc in ipairs(result) do
    local uri = loc.targetUri or loc.uri
    local range = loc.targetSelectionRange or loc.targetRange or loc.range
    if uri and range then
      table.insert(into, { uri = uri, range = range, encoding = encoding })
    end
  end
end

local function client_encoding(client_id)
  local client = vim.lsp.get_client_by_id(client_id)
  return client and client.offset_encoding or 'utf-16'
end

local function sorted_ids(results)
  local ids = vim.tbl_keys(results)
  table.sort(ids)
  return ids
end

-- The picker over places, with the line of each and a preview.
local function open_locations(title, rows, cwd, jump)
  open({
    title = title,
    source = 'items',
    cwd = cwd,
    files = false,
    multi = true,
    items = rows,
    preview = true,
    jump = jump,
    snapshot = true,
  })
end

local function lsp_locations(method, title, references)
  if not enabled() then
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  local win = vim.api.nvim_get_current_win()
  if #vim.lsp.get_clients({ bufnr = buf, method = method }) == 0 then
    vim.notify('No language server supports ' .. title:lower(), vim.log.levels.WARN)
    return
  end
  local cursor = vim.api.nvim_win_get_cursor(win)
  local from = vim.fn.getpos('.')
  from[1] = buf
  local jump = { from = from, tag = vim.fn.expand('<cword>') }
  local cwd = project()
  vim.lsp.buf_request_all(buf, method, function(client)
    local params = vim.lsp.util.make_position_params(win, client.offset_encoding)
    if references then
      params.context = { includeDeclaration = true }
    end
    return params
  end, function(results)
    -- The answer is for where the cursor was.
    if vim.api.nvim_get_current_win() ~= win or vim.api.nvim_get_current_buf() ~= buf then
      return
    end
    local now = vim.api.nvim_win_get_cursor(win)
    if now[1] ~= cursor[1] or now[2] ~= cursor[2] then
      return
    end
    local raw, problem = {}, nil
    for _, id in ipairs(sorted_ids(results)) do
      local res = results[id]
      if res.err then
        problem = problem or res.err.message
      end
      raw_locations(res.result, client_encoding(id), raw)
    end
    local rows = location_rows(raw, cwd)
    if references then
      -- Not the place the cursor is on.
      local here = current_file()
      rows = vim.tbl_filter(function(row)
        local on = here and real(row.path) == here and row.lnum == cursor[1] and row.from0 <= cursor[2]
        return not (on and (cursor[2] < row.to0 or row.to0 <= row.from0))
      end, rows)
    end
    for _, row in ipairs(rows) do
      row.from0, row.to0 = nil, nil
    end
    if #rows == 0 then
      if problem and #raw == 0 then
        vim.notify(problem, vim.log.levels.WARN)
      else
        vim.notify('No ' .. title:lower() .. ' found', vim.log.levels.INFO)
      end
      return
    end
    sort_locations(rows)
    if #rows == 1 then
      -- One answer: go there.
      local row = rows[1]
      open_path(row.path, 'edit', row.lnum, row.col, function()
        pcall(vim.cmd, "normal! m'")
        pcall(vim.fn.settagstack, vim.api.nvim_get_current_win(), { items = { { tagname = jump.tag, from = jump.from } } }, 't')
      end)
      return
    end
    open_locations(title, rows, cwd, jump)
  end)
end

function P.locations(opts)
  if not enabled() then
    return false
  end
  local cwd = project()
  local raw = {}
  raw_locations(opts.locations, opts.encoding or 'utf-16', raw)
  local rows = location_rows(raw, cwd)
  for _, row in ipairs(rows) do
    row.from0, row.to0 = nil, nil
  end
  sort_locations(rows)
  open_locations(opts.title, rows, cwd)
  return true
end

function P.lsp_definitions()
  lsp_locations('textDocument/definition', 'Definitions')
end
function P.lsp_declarations()
  lsp_locations('textDocument/declaration', 'Declarations')
end
function P.lsp_references()
  lsp_locations('textDocument/references', 'References', true)
end
function P.lsp_implementations()
  lsp_locations('textDocument/implementation', 'Implementations')
end
function P.lsp_type_definitions()
  lsp_locations('textDocument/typeDefinition', 'Type definitions')
end

-- ---- Symbols ----

local kind_name = Eshvi.symbols.kind_name

function P.lsp_symbols()
  if not enabled() then
    return
  end
  local method = Eshvi.symbols.method
  local buf = vim.api.nvim_get_current_buf()
  local win = vim.api.nvim_get_current_win()
  if #vim.lsp.get_clients({ bufnr = buf, method = method }) == 0 then
    vim.notify('No language server supports document symbols', vim.log.levels.WARN)
    return
  end
  local cursor = vim.api.nvim_win_get_cursor(win)
  local cwd = project()
  Eshvi.symbols.request(buf, function(tree)
    if vim.api.nvim_get_current_win() ~= win or vim.api.nvim_get_current_buf() ~= buf then
      return
    end
    -- The tree in source order (`Eshvi.symbols`, as the Outline reads it).
    local symbols = Eshvi.symbols.flatten(tree or {})
    if #symbols == 0 then
      vim.notify('No symbols found', vim.log.levels.INFO)
      return
    end
    local name = vim.api.nvim_buf_get_name(buf)
    local path = (name ~= '' and vim.bo[buf].buftype == '') and name or nil
    local items = {}
    local selected = Eshvi.symbols.at(symbols, cursor)
    for i, s in ipairs(symbols) do
      items[i] = {
        text = s.name,
        kind = s.kind,
        -- (A flat list shows its container's name, not how deep it is.)
        depth = s.flat and 0 or s.depth,
        desc = s.container,
        lnum = s.line,
        col = s.col + 1,
        buf = buf,
        path = path,
      }
    end
    open({
      title = 'Symbols',
      source = 'items',
      cwd = cwd,
      files = false,
      multi = true,
      items = items,
      preview = true,
      selected = selected,
      jump = {},
      snapshot = true,
    })
  end)
end

-- Workspace symbols are found by the server for each query, so the app doesn't
-- filter (`live`): it sends the query, the answer names the query it is for.
local WORKSPACE_MAX = 100
function P.lsp_workspace_symbols()
  if not enabled() then
    return
  end
  local method = 'workspace/symbol'
  local buf = vim.api.nvim_get_current_buf()
  if #vim.lsp.get_clients({ bufnr = buf, method = method }) == 0 then
    vim.notify('No language server supports workspace symbols', vim.log.levels.WARN)
    return
  end
  local cwd = project()
  open({
    title = 'Workspace symbols',
    source = 'items',
    cwd = cwd,
    files = false,
    multi = true,
    items = {},
    preview = true,
    live = true,
    hint = 'Type to search symbols',
    jump = {},
    snapshot = true,
    ask = function(text, reply)
      local b = vim.api.nvim_buf_is_valid(buf) and buf or vim.api.nvim_get_current_buf()
      vim.lsp.buf_request_all(b, method, { query = text }, function(results)
        local items, caches = {}, {}
        for _, id in ipairs(sorted_ids(results)) do
          local encoding = client_encoding(id)
          for _, s in ipairs(results[id].result or {}) do
            local loc = s.location
            if loc and loc.uri and #items < WORKSPACE_MAX then
              local path = vim.uri_to_fname(loc.uri)
              local pos = loc.range and loc.range.start or { line = 0, character = 0 }
              if caches[path] == nil then
                caches[path] = lines_for(path) or false
              end
              local line = caches[path] and caches[path][pos.line + 1]
              local rel, full = describe(path, cwd)
              table.insert(items, {
                text = s.name,
                kind = kind_name(s.kind),
                desc = rel,
                path = full,
                lnum = pos.line + 1,
                col = byte_of(line, pos.character, encoding) + 1,
              })
            end
          end
        end
        reply(items)
      end)
    end,
  })
end

-- The app asks for the rows of a live picker's query.
function P.query(id, gen, text)
  local cur = current
  if not cur or cur.id ~= id or not cur.ask then
    return
  end
  cur.ask(text, function(items)
    -- (Replaced, closed or answered by now: nobody wants it.)
    if current ~= cur then
      return
    end
    -- What an accept's place in the list refers to.
    cur.items = items
    vim.rpcnotify(chan, 'gui_picker_items', { id = id, gen = gen, items = items })
  end)
end

-- ---- Diagnostics ----

local function first_line(text)
  return vim.trim((tostring(text or ''):match('^[^\n]*')))
end

function P.diagnostics(opts)
  if not enabled() then
    return
  end
  local only_buffer = opts and opts.buf
  local cwd = project()
  local list = vim.diagnostic.get(only_buffer and vim.api.nvim_get_current_buf() or nil)
  local rows = {}
  for _, d in ipairs(list) do
    local name = vim.api.nvim_buf_get_name(d.bufnr)
    if name ~= '' then
      local rel, full = describe(name, cwd)
      local message = first_line(d.message)
      local lnum, col = d.lnum + 1, d.col + 1
      local to = (d.end_lnum and d.end_lnum ~= d.lnum) and 1000000 or (d.end_col or d.col + 1)
      table.insert(rows, {
        text = message,
        desc = only_buffer and ('Ln ' .. lnum) or (rel .. ':' .. lnum),
        search = message .. ' ' .. rel,
        severity = d.severity,
        path = full,
        buf = d.bufnr,
        lnum = lnum,
        col = col,
        marks = { { d.col, math.max(to, d.col + 1) } },
        rel = rel,
      })
    end
  end
  if #rows == 0 then
    vim.notify('No diagnostics', vim.log.levels.INFO)
    return
  end
  table.sort(rows, function(a, b)
    if a.severity ~= b.severity then
      return a.severity < b.severity
    end
    if a.rel ~= b.rel then
      return a.rel < b.rel
    end
    if a.lnum ~= b.lnum then
      return a.lnum < b.lnum
    end
    return a.col < b.col
  end)
  for _, row in ipairs(rows) do
    row.rel = nil
  end
  open({
    title = only_buffer and 'Buffer diagnostics' or 'Diagnostics',
    source = 'items',
    cwd = cwd,
    files = false,
    multi = true,
    items = rows,
    preview = true,
    jump = {},
  })
end

function P.diagnostics_buffer()
  P.diagnostics({ buf = true })
end

-- ---- Quickfix and location lists ----

-- Go to entry `n` of the list the way `:cc` / `:ll` do, so the list's position
-- moves, in the editor's window.
local function goto_entry(spec, n, how, item)
  enter_editor()
  local split = ({ split = 'split', vsplit = 'vsplit', tab = 'tab split' })[how]
  if split then
    vim.cmd(split)
  end
  if not pcall(vim.cmd, n .. (spec.loclist and 'll' or 'cc')) then
    -- (A location list belongs to its window, which is not this one.)
    open_path(item.path, 'edit', item.lnum, item.col)
    return
  end
  pcall(vim.cmd, 'normal! zv')
end

local function list_picker(loclist)
  if not enabled() then
    return
  end
  local cwd = project()
  local win = vim.api.nvim_get_current_win()
  local what = { items = 1, title = 1 }
  local info = loclist and vim.fn.getloclist(win, what) or vim.fn.getqflist(what)
  if #info.items == 0 then
    vim.notify(loclist and 'Location list is empty' or 'Quickfix list is empty', vim.log.levels.INFO)
    return
  end
  local rows = {}
  for i, it in ipairs(info.items) do
    local text = first_line(it.text)
    local name = (it.bufnr > 0 and vim.api.nvim_buf_is_valid(it.bufnr)) and vim.api.nvim_buf_get_name(it.bufnr) or ''
    if name ~= '' then
      local rel, full = describe(name, cwd)
      rows[i] = {
        text = rel,
        path = full,
        lnum = math.max(it.lnum, 1),
        col = math.max(it.col, 1),
        line = { text = text, ranges = {}, line_ranges = {} },
        search = rel .. ' ' .. text,
      }
    else
      -- A compiler's free text: shown, and not opened.
      rows[i] = { text = text ~= '' and text or '(no text)' }
    end
  end
  local spec
  spec = {
    title = (info.title ~= '' and info.title) or (loclist and 'Location list' or 'Quickfix'),
    source = 'items',
    cwd = cwd,
    files = false,
    multi = true,
    items = rows,
    preview = true,
    loclist = loclist,
    accept = function(c, _, how)
      local item = c.idx and rows[c.idx]
      if item and item.path then
        goto_entry(spec, c.idx, how, item)
      end
    end,
  }
  open(spec)
end

function P.qflist()
  list_picker(false)
end
function P.loclist()
  list_picker(true)
end

-- ---- Neovim's lists, the command palette, resume ----

local function run_ex(command)
  local ok, err = pcall(vim.cmd, command)
  if not ok then
    vim.notify((tostring(err):gsub('^Vim%(%w+%):', '')), vim.log.levels.ERROR)
  end
end

-- Put `:text` on the command line for the user to finish (from any mode).
local function to_cmdline(text)
  local prefix = vim.fn.mode() == 'n' and '' or vim.keycode('<C-\\><C-n>')
  vim.api.nvim_feedkeys(prefix .. ':' .. text, 'n', false)
end

-- A user command as the palette runs it: one that needs an argument is
-- left on the command line to be finished, the others run.
local function run_user_command(name, nargs)
  if nargs == '1' or nargs == '+' then
    to_cmdline(name .. ' ')
  else
    run_ex(name)
  end
end

-- User commands (global, then the buffer's), by name.
local function user_commands()
  local found = vim.api.nvim_get_commands({ builtin = false })
  local ok, buffer = pcall(vim.api.nvim_buf_get_commands, 0, { builtin = false })
  if ok then
    found = vim.tbl_extend('force', found, buffer)
  end
  return found
end

local function one_line(text, max)
  local s = tostring(text):gsub('\r?\n', '↵'):gsub('\t', ' ')
  if #s > max then
    s = s:sub(1, max + vim.str_utf_end(s, max)) .. '…'
  end
  return s
end

local function history_items(kind)
  local rows, seen = {}, {}
  for i = vim.fn.histnr(kind), 1, -1 do
    local entry = vim.fn.histget(kind, i)
    if entry ~= '' and not seen[entry] then
      seen[entry] = true
      table.insert(rows, { text = entry })
    end
  end
  return rows
end

function P.command_history()
  if not enabled() then
    return
  end
  local rows = history_items(':')
  if #rows == 0 then
    return vim.notify('No command history', vim.log.levels.INFO)
  end
  open({
    title = 'Command history',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      local entry = rows[c.idx].text
      vim.fn.histadd(':', entry)
      run_ex(entry)
    end,
  })
end

function P.search_history()
  if not enabled() then
    return
  end
  local rows = history_items('/')
  if #rows == 0 then
    return vim.notify('No search history', vim.log.levels.INFO)
  end
  open({
    title = 'Search history',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      local entry = rows[c.idx].text
      -- Typed, so it searches as typed (offsets, history, 'hlsearch').
      vim.api.nvim_feedkeys('/' .. entry .. '\r', 'n', false)
    end,
  })
end

-- ---- The command palette ----

-- Names run from the palette, newest first (this session only).
local palette_recent = {}

local function remember(name)
  palette_recent = vim.tbl_filter(function(n)
    return n ~= name
  end, palette_recent)
  table.insert(palette_recent, 1, name)
  palette_recent[21] = nil
end

function P.palette()
  if not enabled() then
    return
  end
  local entries = {}
  -- Eshvi's own menus: their items, as "File: Save", with their shortcuts.
  for _, m in ipairs(Eshvi.menu_items and Eshvi.menu_items() or {}) do
    table.insert(entries, {
      row = { text = m.top .. ': ' .. m.name, hint = m.key, gate = m.action },
      run = function()
        run_ex('emenu ' .. m.path)
      end,
    })
  end
  -- User commands: `:Name`, with their description.
  local cmds = user_commands()
  local names = vim.tbl_keys(cmds)
  table.sort(names)
  for _, name in ipairs(names) do
    local info = cmds[name]
    table.insert(entries, {
      row = { text = ':' .. name, desc = info.definition and one_line(info.definition, 120) or '', search = ':' .. name .. ' ' .. (info.definition and one_line(info.definition, 120) or '') },
      run = function()
        run_user_command(name, info.nargs)
      end,
    })
  end
  -- Recently run first, in the order they were run.
  local rank = {}
  for i, n in ipairs(palette_recent) do
    rank[n] = i
  end
  local order = {}
  for i, e in ipairs(entries) do
    order[e] = i
  end
  table.sort(entries, function(a, b)
    local ra, rb = rank[a.row.text] or math.huge, rank[b.row.text] or math.huge
    if ra ~= rb then
      return ra < rb
    end
    return order[a] < order[b]
  end)
  local rows = {}
  for i, e in ipairs(entries) do
    rows[i] = e.row
  end
  open({
    title = 'Command palette',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      local e = entries[c.idx]
      if e then
        remember(e.row.text)
        e.run()
      end
    end,
  })
end

-- Every command Neovim has, built in too. The built-in ones' arguments are
-- unknown, so they are left on the command line; user commands as the palette runs them.
function P.commands()
  if not enabled() then
    return
  end
  local user = user_commands()
  local rows, entries = {}, {}
  for i, name in ipairs(vim.fn.getcompletion('', 'command')) do
    local info = user[name]
    rows[i] = { text = ':' .. name, desc = info and info.definition and one_line(info.definition, 120) or '', search = ':' .. name .. ' ' .. (info and info.definition and one_line(info.definition, 120) or '') }
    entries[i] = { name = name, nargs = info and info.nargs }
  end
  open({
    title = 'Commands',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      local e = entries[c.idx]
      if e.nargs then
        run_user_command(e.name, e.nargs)
      else
        to_cmdline(e.name .. ' ')
      end
    end,
  })
end

-- ---- Keymaps, help, registers ----

local KEYMAP_MODES = { 'n', 'x', 'i', 't', 'c', 'o' }

function P.keymaps()
  if not enabled() then
    return
  end
  local entries = {}
  for _, mode in ipairs(KEYMAP_MODES) do
    for _, buffer in ipairs({ false, true }) do
      local maps = buffer and vim.api.nvim_buf_get_keymap(0, mode) or vim.api.nvim_get_keymap(mode)
      for _, m in ipairs(maps) do
        if not m.lhs:find('^<Plug>') and not m.lhs:find('^<SNR>') then
          local desc = m.desc or m.rhs or (m.callback and '<Lua function>') or ''
          -- (`lhs` has a leader as a bare space; `keytrans` says <Space>.)
          local lhs = vim.fn.keytrans(m.lhsraw or m.lhs)
          table.insert(entries, {
            mode = mode,
            lhs = lhs,
            row = { text = lhs, prefix = mode, desc = one_line(desc, 160), search = lhs .. ' ' .. one_line(desc, 160) },
          })
        end
      end
    end
  end
  local order = {}
  for i, e in ipairs(entries) do
    order[e] = i
  end
  table.sort(entries, function(a, b)
    if a.mode ~= b.mode then
      return order[a] < order[b] -- (the modes in the order given)
    end
    if a.lhs ~= b.lhs then
      return a.lhs < b.lhs
    end
    return order[a] < order[b]
  end)
  local rows = {}
  for i, e in ipairs(entries) do
    rows[i] = e.row
  end
  if #rows == 0 then
    return vim.notify('No keymaps', vim.log.levels.INFO)
  end
  open({
    title = 'Keymaps',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      local e = entries[c.idx]
      if e.mode == 'n' then
        vim.api.nvim_feedkeys(vim.keycode(e.lhs), 'm', false)
      else
        vim.notify('Only normal-mode keymaps can be run from here (' .. e.lhs .. ' is a ' .. e.mode .. ' mapping)', vim.log.levels.INFO)
      end
    end,
  })
end

local help_commands = { edit = 'help', split = 'help', vsplit = 'vertical help', tab = 'tab help' }

-- Every help tag: the `doc/tags` files of the runtime path (`getcompletion('', 'help')`
-- only lists a few dozen for an empty pattern).
local function help_tags()
  local seen, tags = {}, {}
  for _, file in ipairs(vim.api.nvim_get_runtime_file('doc/tags', true)) do
    local ok, lines = pcall(vim.fn.readfile, file)
    for _, line in ipairs(ok and lines or {}) do
      local tag = line:match('^([^\t]+)\t')
      if tag and not seen[tag] then
        seen[tag] = true
        table.insert(tags, tag)
      end
    end
  end
  if #tags == 0 then
    return vim.fn.getcompletion('', 'help')
  end
  table.sort(tags)
  return tags
end

function P.help()
  if not enabled() then
    return
  end
  local tags = help_tags()
  local rows = {}
  for i, tag in ipairs(tags) do
    rows[i] = { text = tag }
  end
  open({
    title = 'Help',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c, _, how)
      run_ex((help_commands[how] or 'help') .. ' ' .. vim.fn.escape(tags[c.idx], ' \\|"'))
    end,
  })
end

local REGISTERS = '"0123456789abcdefghijklmnopqrstuvwxyz-+.:/%#'

function P.registers()
  if not enabled() then
    return
  end
  local rows, names = {}, {}
  for i = 1, #REGISTERS do
    local name = REGISTERS:sub(i, i)
    local ok, content = pcall(vim.fn.getreg, name, 1)
    if ok and content ~= '' then
      table.insert(names, name)
      table.insert(rows, { text = '"' .. name, desc = one_line(content, 200), mono = true, search = '"' .. name .. ' ' .. one_line(content, 200) })
    end
  end
  if #rows == 0 then
    return vim.notify('No registers', vim.log.levels.INFO)
  end
  open({
    title = 'Registers',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    accept = function(c)
      -- Pasted after the cursor, where the picker was opened.
      run_ex('normal! "' .. names[c.idx] .. 'p')
    end,
  })
end

-- ---- Marks and jumps ----

-- The text of line `lnum` of a place, from the buffer if it is loaded, else the file.
local function line_at(buf, path, lnum)
  if buf and buf > 0 and vim.api.nvim_buf_is_loaded(buf) then
    return (vim.api.nvim_buf_get_lines(buf, lnum - 1, lnum, false)[1])
  end
  local lines = path and path ~= '' and lines_for(path)
  return lines and lines[lnum] or nil
end

local function place_row(cwd, path, buf, lnum, col)
  local rel, full
  if path and path ~= '' then
    rel, full = describe(path, cwd)
  end
  return rel, full
end

function P.marks()
  if not enabled() then
    return
  end
  local cwd = project()
  local here = vim.api.nvim_get_current_buf()
  local rows, seen = {}, {}
  local function add(m, buf, path)
    local lnum, col = m.pos[2], m.pos[3]
    local mark = m.mark:sub(2)
    if lnum < 1 or seen[mark] then
      return
    end
    seen[mark] = true
    local rel, full = place_row(cwd, path)
    local text = line_at(buf, path, lnum)
    local where = (rel or '[No Name]') .. ':' .. lnum
    local line = text and vim.trim(text) or ''
    table.insert(rows, {
      text = "'" .. mark,
      desc = where .. '  ' .. one_line(line, 200),
      search = "'" .. mark .. ' ' .. where .. ' ' .. line,
      path = full,
      buf = buf,
      lnum = lnum,
      col = math.max(col, 1),
    })
  end
  for _, m in ipairs(vim.fn.getmarklist(here)) do
    add(m, here, vim.api.nvim_buf_get_name(here))
  end
  for _, m in ipairs(vim.fn.getmarklist()) do
    local buf = m.pos[1]
    add(m, buf > 0 and buf or nil, m.file and vim.fn.fnamemodify(m.file, ':p') or nil)
  end
  if #rows == 0 then
    return vim.notify('No marks', vim.log.levels.INFO)
  end
  open({
    title = 'Marks',
    source = 'items',
    cwd = cwd,
    files = false,
    multi = false,
    items = rows,
    preview = true,
    jump = {},
  })
end

function P.jumps()
  if not enabled() then
    return
  end
  local cwd = project()
  local list, curr = unpack(vim.fn.getjumplist())
  local entries = {}
  for i = #list, 1, -1 do
    local j = list[i]
    local name = (j.bufnr > 0 and vim.api.nvim_buf_is_valid(j.bufnr)) and vim.api.nvim_buf_get_name(j.bufnr) or j.filename or ''
    if j.lnum >= 1 then
      local rel, full = place_row(cwd, name)
      local text = line_at(j.bufnr, name, j.lnum)
      local where = (rel or '[No Name]') .. ':' .. j.lnum
      local line = text and vim.trim(text) or ''
      table.insert(entries, {
        at = i,
        row = {
          text = where,
          desc = one_line(line, 200),
          search = where .. ' ' .. line,
          path = full,
          buf = j.bufnr > 0 and j.bufnr or nil,
          lnum = j.lnum,
          col = j.col + 1,
        },
      })
    end
  end
  if #entries == 0 then
    return vim.notify('No jumps', vim.log.levels.INFO)
  end
  local rows = {}
  for i, e in ipairs(entries) do
    rows[i] = e.row
  end
  open({
    title = 'Jumps',
    source = 'items',
    cwd = cwd,
    files = false,
    multi = false,
    items = rows,
    preview = true,
    accept = function(c, _, how)
      local e = entries[c.idx]
      if how ~= 'edit' then
        local row = e.row
        if row.path then
          open_path(row.path, how, row.lnum, row.col)
        end
        return
      end
      -- Go there as Ctrl-O and Ctrl-I do, so the jumplist moves with us.
      enter_editor()
      local steps = curr - (e.at - 1)
      if steps > 0 then
        pcall(vim.cmd, 'normal! ' .. steps .. vim.keycode('<C-o>'))
      elseif steps < 0 then
        pcall(vim.cmd, 'normal! ' .. -steps .. vim.keycode('<C-i>'))
      end
    end,
  })
end

-- ---- Undo history ----

local function ago(time)
  local d = os.time() - time
  if d < 5 then
    return 'just now'
  elseif d < 60 then
    return d .. ' sec ago'
  elseif d < 3600 then
    return math.floor(d / 60) .. ' min ago'
  elseif d < 86400 then
    return math.floor(d / 3600) .. ' hr ago'
  end
  local days = math.floor(d / 86400)
  return days .. (days == 1 and ' day ago' or ' days ago')
end

function P.undo()
  if not enabled() then
    return
  end
  local tree = vim.fn.undotree()
  local nodes = {}
  -- Alternative branches hang off the state before the entry they sit in.
  local function walk(entries, parent)
    for _, e in ipairs(entries) do
      table.insert(nodes, { seq = e.seq, time = e.time, parent = parent })
      if e.alt then
        walk(e.alt, parent)
      end
      parent = e.seq
    end
  end
  walk(tree.entries or {}, 0)
  if #nodes == 0 then
    return vim.notify('No undo history', vim.log.levels.INFO)
  end
  table.sort(nodes, function(a, b)
    return a.seq > b.seq
  end)
  local rows, selected = {}, nil
  for i, n in ipairs(nodes) do
    local when = ago(n.time)
    rows[i] = { text = tostring(n.seq), desc = when, hint = '+' .. math.max(n.seq - n.parent, 1), search = n.seq .. ' ' .. when }
    if n.seq == tree.seq_cur then
      selected = i
    end
  end
  open({
    title = 'Undo history',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    selected = selected,
    accept = function(c)
      run_ex('undo ' .. nodes[c.idx].seq)
    end,
  })
end

-- ---- Git history of the file, or of the line ----
--
-- There is no native view of one file's or one line's history (the Source
-- Control Graph is the repository's), so these are pickers. `git` runs through
-- `vim.system` with a callback: the editor never waits for it. Enter opens the
-- file at that commit the way the Graph opens a file under a commit: the GUI is
-- asked for it (`gui_git_diff`), since `[git] diff` says how a diff looks.

local RS, US = '\30', '\31'
local LOG_FORMAT = '--format=' .. RS .. '%H' .. US .. '%P' .. US .. '%an' .. US .. '%at' .. US .. '%s'

local function git_run(top, args, done)
  local cmd = { 'git', '-c', 'core.quotepath=off', '-C', top }
  vim.list_extend(cmd, args)
  local ok, err = pcall(vim.system, cmd, { text = true }, vim.schedule_wrap(done))
  if not ok then
    vim.notify('git: ' .. tostring(err), vim.log.levels.ERROR)
  end
end

local function git_error(r)
  if (r.stderr or ''):find('does not have any commits yet', 1, true) then
    return vim.notify('Git history: no commits yet', vim.log.levels.INFO)
  end
  local line = (r.stderr or ''):match('[^\n]+') or ('exit code ' .. tostring(r.code))
  vim.notify('git log: ' .. line, vim.log.levels.ERROR)
end

-- The commits in `git log`'s output, newest first, with the path the file had
-- in each and the one before it. `follow`: from `--name-status` (a rename
-- changes the path from there back); else (`-L`) from the patch's headers.
local function parse_history(out, path, follow)
  local entries = {}
  local cur = path
  for record in out:gmatch(RS .. '([^' .. RS .. ']*)') do
    local head, rest = record:match('^([^\n]*)\n?(.*)$')
    local f = vim.split(head, US, { plain = true })
    local e = {
      oid = f[1],
      parent = (f[2] or ''):match('%S+') or '',
      author = f[3] or '',
      time = tonumber(f[4]) or 0,
      subject = f[5] or '',
      path = cur,
      deleted = false,
    }
    if follow then
      for line in rest:gmatch('[^\n]+') do
        local status, old, new = line:match('^(%a)%d*\t([^\t]+)\t(.+)$')
        if status then
          e.path, e.left_path = new, old
          cur = old
          break
        end
        status, old = line:match('^(%a)\t(.+)$')
        if status then
          e.path, e.deleted = old, status == 'D'
          break
        end
      end
    else
      local left
      for line in rest:gmatch('[^\n]+') do
        if line:sub(1, 2) == '@@' then
          break
        end
        left = line:match('^%-%-%- a/(.+)$') or left
        e.path = line:match('^%+%+%+ b/(.+)$') or e.path
      end
      if left and left ~= e.path then
        e.left_path = left
      end
    end
    table.insert(entries, e)
  end
  return entries
end

local function history_picker(top, title, entries)
  local rows = {}
  for i, e in ipairs(entries) do
    local author = e.author ~= '' and (e.author .. ', ') or ''
    local short = e.oid:sub(1, 7)
    rows[i] = {
      text = e.subject,
      prefix = short,
      desc = author .. ago(e.time),
      search = e.subject .. ' ' .. short .. ' ' .. e.author,
    }
  end
  open({
    title = title,
    source = 'items',
    cwd = top,
    files = false,
    multi = false,
    no_resume = true,
    items = rows,
    accept = function(c)
      local e = entries[c.idx]
      if e then
        vim.rpcnotify(chan, 'gui_git_diff', {
          top = top,
          oid = e.oid,
          parent = e.parent,
          path = e.path,
          left_path = e.left_path,
          deleted = e.deleted,
        })
      end
    end,
  })
end

-- The file of the current buffer, its repository and its path in it, then
-- `go(top, rel)`; or a message saying why not.
local function with_tracked_file(go)
  local buf = vim.api.nvim_get_current_buf()
  local name = vim.api.nvim_buf_get_name(buf)
  if vim.bo[buf].buftype ~= '' or name == '' then
    return vim.notify('Git history: this buffer is not a file', vim.log.levels.INFO)
  end
  local file = real(name)
  local dir = vim.fs.dirname(file)
  if vim.fn.isdirectory(dir) ~= 1 then
    return vim.notify('Git history: not in a git repository', vim.log.levels.INFO)
  end
  git_run(dir, { 'rev-parse', '--show-toplevel' }, function(r)
    local top = r.code == 0 and (r.stdout or ''):match('[^\n]+')
    if not top then
      return vim.notify('Git history: not in a git repository', vim.log.levels.INFO)
    end
    top = real(top)
    if file:sub(1, #top + 1) ~= top .. '/' then
      return vim.notify('Git history: not in a git repository', vim.log.levels.INFO)
    end
    local rel = file:sub(#top + 2)
    git_run(top, { 'ls-files', '--error-unmatch', '--', rel }, function(t)
      if t.code ~= 0 then
        return vim.notify('Git history: ' .. rel .. ' is not tracked by git', vim.log.levels.INFO)
      end
      go(top, rel)
    end)
  end)
end

function P.git_log_file()
  if not enabled() then
    return
  end
  with_tracked_file(function(top, rel)
    git_run(top, { 'log', '--follow', '--name-status', '--no-color', '--no-ext-diff', LOG_FORMAT, '--', rel }, function(r)
      if r.code ~= 0 then
        return git_error(r)
      end
      local entries = parse_history(r.stdout or '', rel, true)
      if #entries == 0 then
        return vim.notify('Git history: no commits for ' .. rel, vim.log.levels.INFO)
      end
      history_picker(top, 'History of ' .. rel, entries)
    end)
  end)
end

function P.git_log_line()
  if not enabled() then
    return
  end
  local lnum = vim.api.nvim_win_get_cursor(0)[1]
  with_tracked_file(function(top, rel)
    local range = ('-L%d,%d:%s'):format(lnum, lnum, rel)
    git_run(top, { 'log', range, '-U0', '--no-color', '--no-ext-diff', LOG_FORMAT }, function(r)
      if r.code ~= 0 then
        return git_error(r)
      end
      local entries = parse_history(r.stdout or '', rel, false)
      if #entries == 0 then
        return vim.notify(('Git history: no commits for line %d of %s'):format(lnum, rel), vim.log.levels.INFO)
      end
      history_picker(top, ('History of %s:%d'):format(rel, lnum), entries)
    end)
  end)
end

-- ---- Colorschemes, tried on as the selection moves ----

function P.colorschemes()
  if not enabled() then
    return
  end
  local names = vim.fn.getcompletion('', 'color')
  local original = { name = vim.g.colors_name, background = vim.o.background }
  local applied = original.name
  local rows, selected = {}, nil
  for i, name in ipairs(names) do
    rows[i] = { text = name }
    if name == original.name then
      selected = i
    end
  end
  local function apply(name)
    if name and name ~= applied then
      applied = name
      pcall(vim.cmd.colorscheme, name)
    end
  end
  open({
    title = 'Colorschemes',
    source = 'items',
    cwd = project(),
    files = false,
    multi = false,
    items = rows,
    selected = selected,
    no_resume = true,
    -- The whole interface follows the scheme; the background is left to the
    -- scheme (it changes it if it means to).
    on_select = function(idx)
      apply(names[idx])
    end,
    accept = function(c)
      apply(names[c.idx])
    end,
    on_cancel = function()
      if applied ~= original.name then
        pcall(vim.cmd.colorscheme, original.name or 'default')
        if vim.o.background ~= original.background then
          vim.o.background = original.background
        end
      end
    end,
  })
end

-- ---- Resume ----

function P.resume()
  if not enabled() then
    return
  end
  local l = last
  if not l then
    return vim.notify('No picker to resume', vim.log.levels.INFO)
  end
  resume_state = l.state
  if l.resume.snapshot then
    local spec = vim.tbl_extend('force', {}, l.spec)
    spec.id, spec.done, spec.opened, spec.resume = nil, nil, nil, nil
    open(spec)
  else
    building = l.resume
    local ok, err = pcall(l.resume.run)
    building = nil
    if not ok then
      resume_state = nil
      error(err, 0)
    end
  end
  resume_state = nil
end

-- What `resume` can call again: the sources that read what Neovim knows.
for _, name in ipairs({
  'files', 'git_files', 'smart', 'buffers', 'recent', 'lines', 'diagnostics', 'diagnostics_buffer', 'qflist', 'loclist',
  'palette', 'command_history', 'search_history', 'commands', 'keymaps', 'help', 'registers', 'marks', 'jumps', 'undo',
}) do
  P[name] = resumable(P[name])
end

local function install()
  if vim.g.eshvi_native_picker == false then
    return
  end
  vim.ui.select = P.select
end
if vim.v.vim_did_enter == 1 then
  install()
else
  vim.api.nvim_create_autocmd('VimEnter', { group = group, once = true, callback = install })
end
-- lazy.nvim's VeryLazy comes after VimEnter, where plugins set theirs.
vim.api.nvim_create_autocmd('User', { group = group, pattern = 'VeryLazy', once = true, callback = install })

vim.api.nvim_create_autocmd('BufEnter', {
  group = group,
  callback = function(ev)
    local name = vim.api.nvim_buf_get_name(ev.buf)
    if name ~= '' and vim.bo[ev.buf].buftype == '' then
      session = vim.tbl_filter(function(p)
        return p ~= name
      end, session)
      table.insert(session, 1, name)
      session[101] = nil
    end
  end,
})
vim.api.nvim_create_autocmd('VimLeavePre', { group = group, callback = P.close })

local sources = {
  'files', 'git_files', 'smart', 'buffers', 'recent', 'grep', 'grep_word', 'lines', 'grep_buffers',
  'lsp_definitions', 'lsp_declarations', 'lsp_references', 'lsp_implementations', 'lsp_type_definitions',
  'lsp_symbols', 'lsp_workspace_symbols', 'diagnostics', 'diagnostics_buffer', 'qflist', 'loclist',
  'palette', 'command_history', 'search_history', 'commands', 'keymaps', 'help', 'registers', 'marks', 'jumps', 'undo',
  'colorschemes', 'git_log_file', 'git_log_line', 'resume',
}
vim.api.nvim_create_user_command('EshviPick', function(o)
  local source = o.args
  if not P[source] or not vim.tbl_contains(sources, source) then
    vim.notify('EshviPick: ' .. table.concat(sources, ', '), vim.log.levels.ERROR)
    return
  end
  P[source]()
end, {
  nargs = 1,
  complete = function()
    return sources
  end,
})
