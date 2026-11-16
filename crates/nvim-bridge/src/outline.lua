-- The Outline: the symbols of the file you are in (the language server's
-- document symbols), or a Markdown file's headings, as a tree for the
-- Explorer's Outline pane (`Eshvi.tree`, `tree.lua`). Loaded with any config,
-- after `tree.lua` and `symbols.lua`.
--
--   Eshvi.outline.configure({ enabled = true, follow_cursor = true })
--                                      `[outline]` in config.toml; the app sends it
--   Eshvi.outline.refresh()            read the file again
--
-- Which file: the current window's when it is a normal file buffer; a Markdown
-- preview beside it counts as the file it previews; anything else (a panel, a
-- terminal, a float) keeps the last one.
local chan = ...

_G.Eshvi = _G.Eshvi or {}
local O = {}
Eshvi.outline = O

local VIEW = 'outline'
-- Between the names of a node's ancestors in its id.
local SEP = '\31'

local cfg = { enabled = false, follow_cursor = true }
local group = vim.api.nvim_create_augroup('eshvi_outline', { clear = true })

-- buf: the file shown; tick: the changedtick last refreshed for; shown: the one
-- of the answer on show; syms: its nodes in source order; followed: the id the
-- cursor is on; seq: the latest request.
local state = { buf = nil, tick = nil, shown = nil, syms = nil, followed = nil, accepted = nil, seq = 0, message = nil }

local function valid(buf)
  return buf ~= nil and vim.api.nvim_buf_is_valid(buf)
end

-- The file window `win` stands for: its buffer, or the one a preview beside it shows.
local function file_of(win)
  if not vim.api.nvim_win_is_valid(win) or vim.api.nvim_win_get_config(win).relative ~= '' then
    return nil
  end
  local of = vim.w[win].eshvi_preview_of
  if of then
    return valid(of) and of or nil
  end
  local buf = vim.api.nvim_win_get_buf(win)
  if vim.bo[buf].buftype ~= '' then
    return nil
  end
  -- (The empty buffer Neovim starts with is no file.)
  if vim.api.nvim_buf_get_name(buf) == '' and not vim.bo[buf].modified and vim.api.nvim_buf_line_count(buf) == 1
    and vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1] == '' then
    return nil
  end
  return buf
end

-- The file to show: the current window's, else the last one while it lives.
local function pick()
  return file_of(vim.api.nvim_get_current_win()) or (valid(state.buf) and state.buf or nil)
end

local function one_line(text)
  return vim.trim((tostring(text):gsub('%s+', ' ')))
end

-- ---- Nodes ----

-- Ids are the names from the root joined, with `#n` for the n-th of a name and
-- kind among its siblings (overloads), so they survive an edit elsewhere.
local function id_for(parent_id, name, kind, seen)
  local key = name .. SEP .. kind
  seen[key] = (seen[key] or 0) + 1
  local id = (parent_id and (parent_id .. SEP) or '') .. name
  if seen[key] > 1 then
    id = id .. '#' .. seen[key]
  end
  return id
end

-- The tree's nodes for `Eshvi.tree.set`, from symbols (or headings): the
-- source's own fields (`col`, `container`) stay with the node.
local function nodes_of(list, parent_id, out)
  local nodes, seen = {}, {}
  for _, s in ipairs(list) do
    local label = one_line(s.name)
    local id = id_for(parent_id, label, s.kind, seen)
    s.id = id
    table.insert(out, s)
    local node = {
      id = id,
      label = label,
      hint = s.detail and s.detail ~= '' and one_line(s.detail) or nil,
      kind = s.kind,
      line = s.line,
      end_line = s.end_line,
      col = s.col,
      children = nodes_of(s.children or {}, id, out),
    }
    table.insert(nodes, node)
  end
  return nodes
end

-- ---- Markdown ----

local function sub(s, pattern, repl)
  return (s:gsub(pattern, repl))
end

-- A heading's text without its markup: emphasis, code, links, a closing run of #.
local function plain(text)
  text = sub(text, '\r', '')
  text = sub(text, '%s*\n%s*', ' ')
  text = sub(text, '!%[([^%]]*)%]%b()', '%1')
  text = sub(text, '%[([^%]]*)%]%b()', '%1')
  text = sub(text, '%[([^%]]*)%]%[[^%]]*%]', '%1')
  text = sub(text, '`+([^`]+)`+', '%1')
  text = sub(text, '%*%*(.-)%*%*', '%1')
  text = sub(text, '__(.-)__', '%1')
  text = sub(text, '%*(.-)%*', '%1')
  -- (An underscore inside a word is part of it.)
  text = sub(' ' .. text .. ' ', '([^%w_])_([^_]-)_([^%w_])', '%1%2%3')
  text = sub(text, '~~(.-)~~', '%1')
  text = sub(vim.trim(text), '%s+#+$', '')
  if text:match('^#+$') then
    text = ''
  end
  return vim.trim(text)
end

local markdown_query
-- The headings of a Markdown buffer, by Neovim's own parser (code blocks and
-- front matter are other nodes): { name, kind, level, line }, in order.
local function headings(buf)
  local ok, parser = pcall(vim.treesitter.get_parser, buf, 'markdown')
  if not ok or not parser then
    return nil
  end
  local trees = parser:parse()
  if not trees or not trees[1] then
    return nil
  end
  markdown_query = markdown_query or vim.treesitter.query.parse('markdown', '[(atx_heading) (setext_heading)] @heading')
  local out = {}
  for _, node in markdown_query:iter_captures(trees[1]:root(), buf) do
    local level, text
    for child in node:iter_children() do
      local t = child:type()
      level = level or tonumber(t:match('^atx_h(%d)_marker$') or t:match('^setext_h(%d)_underline$'))
      if t == 'inline' or t == 'paragraph' then
        text = vim.treesitter.get_node_text(child, buf)
      end
    end
    local name = text and plain(text) or ''
    if level and name ~= '' then
      table.insert(out, { name = name, kind = 'H' .. level, level = level, line = node:start() + 1, col = 0 })
    end
  end
  return out
end

-- Headings nested by level: an H3 after an H1 with no H2 is the H1's child.
local function heading_tree(list)
  local roots, stack = {}, {}
  for _, h in ipairs(list) do
    h.children = {}
    while #stack > 0 and stack[#stack].level >= h.level do
      table.remove(stack)
    end
    table.insert(#stack > 0 and stack[#stack].children or roots, h)
    table.insert(stack, h)
  end
  return roots
end

-- ---- Showing ----

local function show(tree, message, buf, tick)
  state.syms = {}
  local nodes = nodes_of(tree, nil, state.syms)
  state.shown = tick
  state.followed = nil
  state.message = message
  Eshvi.tree.set(VIEW, nodes, { message = message, expand = 1, buf = buf })
end

local function message_only(text)
  state.syms = nil
  state.shown = nil
  state.followed = nil
  if state.message ~= text or state.sent_buf ~= state.buf then
    state.message = text
    state.sent_buf = state.buf
    Eshvi.tree.set(VIEW, {}, { message = text, buf = state.buf })
  end
end

local follow

-- Read the file again (`force`: even if it is the one shown, unchanged).
local function refresh(force)
  if not cfg.enabled then
    return
  end
  local buf = pick()
  if not buf then
    state.buf, state.tick = nil, nil
    return message_only('No file open')
  end
  local tick = vim.api.nvim_buf_get_changedtick(buf)
  if buf == state.buf and tick == state.tick and not force then
    return
  end
  if buf ~= state.buf then
    state.buf, state.shown, state.syms, state.followed = buf, nil, nil, nil
  end
  state.tick = tick
  state.seq = state.seq + 1
  local seq = state.seq
  local function done(tree, message)
    -- Not for a file no longer the outline's, nor older than what is shown.
    if seq ~= state.seq or state.buf ~= buf or (state.shown and tick < state.shown) then
      return
    end
    if tree then
      show(tree, message, buf, tick)
    else
      message_only(message)
    end
    follow()
  end
  -- Markdown: always its headings, with a server attached or not.
  if vim.bo[buf].filetype == 'markdown' then
    local list = headings(buf)
    if not list then
      return done(nil, 'No outline: no Markdown parser')
    end
    local tree = heading_tree(list)
    return done(tree, #tree == 0 and 'No headings found' or nil)
  end
  Eshvi.symbols.request(buf, function(tree)
    if not tree then
      return done(nil, 'No outline: no language server for this file')
    end
    done(tree, #tree == 0 and 'No symbols found' or nil)
  end)
end

-- ---- Following the cursor ----

local follow_timer = nil

-- Select the node the cursor is in.
function follow()
  local win = vim.api.nvim_get_current_win()
  if not (cfg.enabled and cfg.follow_cursor and state.syms and #state.syms > 0) then
    return
  end
  if vim.w[win].eshvi_preview_of or vim.api.nvim_win_get_buf(win) ~= state.buf then
    return
  end
  local cursor = vim.api.nvim_win_get_cursor(win)
  -- Put there by choosing a node: that one is the selection, not what the
  -- rule would pick (a parameter's function).
  local chosen = state.accepted
  if chosen and chosen.line == cursor[1] and chosen.col == cursor[2] then
    if chosen.id ~= state.followed then
      state.followed = chosen.id
      Eshvi.tree.select(VIEW, chosen.id)
    end
    return
  end
  local at
  if vim.bo[state.buf].filetype == 'markdown' then
    for i, h in ipairs(state.syms) do
      if h.line <= cursor[1] then
        at = i
      end
    end
  else
    at = Eshvi.symbols.at(state.syms, cursor)
  end
  local id = at and state.syms[at].id
  if id and id ~= state.followed then
    state.followed = id
    Eshvi.tree.select(VIEW, id)
  end
end

local function follow_later()
  if follow_timer then
    return
  end
  follow_timer = true
  vim.defer_fn(function()
    follow_timer = nil
    follow()
  end, 100)
end

-- ---- Accepting ----

-- The window to move: the one showing the file (not a preview of it); else the
-- editor's window, given the file.
local function target(buf)
  local function shows(w)
    return vim.api.nvim_win_get_buf(w) == buf and vim.api.nvim_win_get_config(w).relative == '' and not vim.w[w].eshvi_preview_of
  end
  local cur = vim.api.nvim_get_current_win()
  if shows(cur) then
    return cur
  end
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if shows(w) then
      return w
    end
  end
  local editor = Eshvi.editor_window()
  if editor then
    vim.api.nvim_win_set_buf(editor, buf)
  end
  return editor
end

-- The cursor to the node, the jumplist keeping where it was. Whether the
-- keyboard goes to the editor is the app's (open, peek).
local function accept(node)
  local buf = state.buf
  if not valid(buf) or not node.line then
    return
  end
  local win = target(buf)
  if not win then
    return
  end
  vim.api.nvim_set_current_win(win)
  vim.cmd("normal! m'")
  local off = node.line < vim.fn.line('w0') or node.line > vim.fn.line('w$')
  pcall(vim.api.nvim_win_set_cursor, win, { math.min(node.line, vim.api.nvim_buf_line_count(buf)), node.col or 0 })
  vim.cmd('normal! zv')
  if off then
    vim.cmd('normal! zz')
  end
  -- (The cursor is where the node is: `follow` keeps it selected.)
  state.accepted = { id = node.id, line = vim.api.nvim_win_get_cursor(win)[1], col = vim.api.nvim_win_get_cursor(win)[2] }
  state.followed = node.id
end

-- ---- Wiring ----

local typing = nil
local function after_typing()
  if typing then
    return
  end
  typing = true
  vim.defer_fn(function()
    typing = nil
    refresh(false)
  end, 500)
end

local function wire()
  vim.api.nvim_clear_autocmds({ group = group })
  local function on(events, fn, opts)
    vim.api.nvim_create_autocmd(events, vim.tbl_extend('force', { group = group, callback = fn }, opts or {}))
  end
  on({ 'BufEnter', 'WinEnter', 'BufWritePost', 'InsertLeave' }, function()
    vim.schedule(function()
      refresh(false)
    end)
  end)
  on({ 'LspAttach', 'FileType' }, function()
    vim.schedule(function()
      refresh(true)
    end)
  end)
  -- (The client is still attached when this fires.)
  on('LspDetach', function()
    vim.defer_fn(function()
      refresh(true)
    end, 50)
  end)
  on({ 'TextChanged', 'TextChangedI' }, after_typing)
  on({ 'CursorMoved', 'CursorMovedI' }, follow_later)
  on({ 'BufWipeout', 'BufDelete' }, function(ev)
    if ev.buf == state.buf then
      vim.schedule(function()
        refresh(true)
      end)
    end
  end)
end

function O.refresh()
  refresh(true)
end

-- (For the tests and for timing.)
O._headings = headings

function O.configure(c)
  if c.follow_cursor ~= nil then
    local was = cfg.follow_cursor
    cfg.follow_cursor = c.follow_cursor and true or false
    if cfg.follow_cursor and not was then
      state.followed = nil
      follow()
    end
  end
  if c.enabled ~= nil and (c.enabled and true or false) ~= cfg.enabled then
    cfg.enabled = c.enabled and true or false
    if cfg.enabled then
      Eshvi.tree.register(VIEW, {
        accept = function(node)
          accept(node)
        end,
      })
      wire()
      state.buf, state.tick, state.message = nil, nil, nil
      refresh(true)
    else
      vim.api.nvim_clear_autocmds({ group = group })
      Eshvi.tree.unregister(VIEW)
      state.buf, state.tick, state.syms, state.message = nil, nil, nil, nil
    end
  end
end

O.configure({ enabled = true })
