-- Neovim's own completion, as eshvi's own config has it (`[neovim] config =
-- "own"`): the menu while typing in insert mode and in the command line, LSP
-- items first, paths, and documentation on request (Ctrl-Space). The GUI draws
-- the menu (`ext_popupmenu`). Set up on VimEnter, so the config can opt out
-- first: `vim.g.eshvi_native_completion = false`.
local chan = ...

local MAX_VISIBLE = 12 -- the GUI's menu height (popupmenu.rs)
local DESC = 'eshvi: '

-- Paths while typing -------------------------------------------------------

-- The path-like word before the cursor: it has a "/" (this includes "./",
-- "../" and "~/"), apart from comment slashes ("//", "/*"). Returns the
-- word's start column (0-based) and the word.
local function path_word()
  local line = vim.api.nvim_get_current_line():sub(1, vim.api.nvim_win_get_cursor(0)[2])
  local word = line:match('[^%s"\'`()%[%]{}<>|,;=]*$')
  if not word:find('/', 1, true) or word:find('^/+$') or word:find('^//') or word:find('^/%*') then
    return nil
  end
  return #line - #word, word
end

-- The folder a path word names (up to its last "/"): "~/" is the home folder,
-- relative paths are from the buffer's folder (not the working directory).
local function path_dir(word)
  local dir = word:match('^(.*/)')
  if dir:sub(1, 1) == '~' then
    dir = vim.uv.os_homedir() .. dir:sub(2)
  elseif dir:sub(1, 1) ~= '/' then
    local here = vim.fn.expand('%:p:h')
    dir = (here == '' and vim.uv.cwd() or here) .. '/' .. dir
  end
  return dir
end

-- 'complete' function source: answers only for a path-like word.
function Eshvi.complete_path(findstart, base)
  local col, word = path_word()
  if findstart == 1 then
    -- Not a path: no source start here, nothing else is disturbed.
    return col and (col + #word:match('^(.*/)')) or -3
  end
  if not col then
    return {}
  end
  local dir = path_dir(word)
  local handle = vim.uv.fs_scandir(dir)
  if not handle then
    return {}
  end
  local dirs, files = {}, {}
  local hidden = base:sub(1, 1) == '.'
  while true do
    local name, kind = vim.uv.fs_scandir_next(handle)
    if not name then
      break
    end
    if hidden or name:sub(1, 1) ~= '.' then
      if kind == 'link' then
        local stat = vim.uv.fs_stat(dir .. name)
        kind = stat and stat.type or kind
      end
      table.insert(kind == 'directory' and dirs or files, name)
    end
  end
  table.sort(dirs)
  table.sort(files)
  local items = {}
  for _, name in ipairs(dirs) do
    table.insert(items, { word = name .. '/', kind = 'Folder', user_data = { eshvi_path = true } })
  end
  for _, name in ipairs(files) do
    table.insert(items, { word = name, kind = 'File', user_data = { eshvi_path = true } })
  end
  return items
end

local PATH_SOURCE = 'Fv:lua.Eshvi.complete_path'
local DEFAULT_SOURCES = PATH_SOURCE .. ',.^5,w^5,b^5'
-- With a language server: its items first, buffer words only after them.
local LSP_SOURCES = PATH_SOURCE .. ',o^20,.^3,w^3,b^3'

-- Paths come first, then the items the language server gave: Neovim puts the
-- buffer's words, found sooner, ahead of them. Within each: the items that
-- start with the word typed (with its case, then without), then the other
-- (fuzzy) matches; each in the server's order (its sortText), else
-- alphabetical. ('nosort' keeps this order while typing narrows the menu; the
-- server's order alone put rust-analyzer's `split_inclusive` above `iter`
-- for "ite".)
local function rank(item)
  if vim.tbl_get(item, 'user_data', 'eshvi_path') then
    return 0
  end
  return vim.tbl_get(item, 'user_data', 'nvim', 'lsp', 'completion_item') and 1 or 2
end

-- The word before the cursor, once per sort (cleared after it).
local typed
local function typed_word()
  if not typed then
    local col = vim.api.nvim_win_get_cursor(0)[2]
    typed = vim.fn.matchstr(vim.api.nvim_get_current_line():sub(1, col), [[\k*$]])
    vim.schedule(function()
      typed = nil
    end)
  end
  return typed
end

-- 0: starts with the word typed, 1: so without its case, 2: neither.
local function closeness(text, word)
  if word == '' or vim.startswith(text, word) then
    return 0
  end
  return vim.startswith(text:lower(), word:lower()) and 1 or 2
end

local function lsp_first(a, b)
  local ra, rb = rank(a), rank(b)
  if ra ~= rb then
    return ra < rb
  end
  local ia = vim.tbl_get(a, 'user_data', 'nvim', 'lsp', 'completion_item')
  local ib = vim.tbl_get(b, 'user_data', 'nvim', 'lsp', 'completion_item')
  local word = typed_word()
  local ca = closeness(ia and (ia.filterText or ia.label) or a.word, word)
  local cb = closeness(ib and (ib.filterText or ib.label) or b.word, word)
  if ca ~= cb then
    return ca < cb
  elseif ra == 1 then
    return (ia.sortText or ia.label) < (ib.sortText or ib.label)
  end
  return a.word < b.word
end

-- Documentation on request ---------------------------------------------------

-- The state of the docs shown beside the menu: `open` between two
-- Ctrl-Space presses; `token` tells a late answer from the current item's.
local docs = { open = false, token = 0, timer = nil }

local function send_doc(selected, contents)
  vim.rpcnotify(chan, 'gui_pum_doc', { selected = selected, contents = contents })
end

-- The selected item and its index in the menu. (With `matches` asked for,
-- complete_info's `selected` is an index into them, the items shown; without,
-- into all the candidates, filtered out ones too.)
local function selected_item()
  local info = vim.fn.complete_info({ 'selected', 'matches' })
  local item = (info.selected or -1) >= 0 and info.matches and info.matches[info.selected + 1]
  if not item then
    return nil, -1
  end
  return item, info.selected
end

-- Markdown for `item` (a completion item of Vim's), calling `done` with it.
-- An LSP item is resolved first when its server can.
local function item_markdown(item, done)
  local lsp_item = vim.tbl_get(item, 'user_data', 'nvim', 'lsp', 'completion_item')
  if not lsp_item then
    return done(vim.trim(item.info or ''))
  end
  local client = vim.lsp.get_client_by_id(vim.tbl_get(item, 'user_data', 'nvim', 'lsp', 'client_id') or -1)
  local provider = client and client.server_capabilities.completionProvider
  if not (provider and provider.resolveProvider) then
    return done(Eshvi.item_markdown(lsp_item))
  end
  client:request('completionItem/resolve', lsp_item, function(err, resolved)
    done(Eshvi.item_markdown(err == nil and resolved or lsp_item))
  end, vim.api.nvim_get_current_buf())
end

-- Ask for the selected item's docs (or, with nothing selected, clear them).
local function request_doc()
  docs.token = docs.token + 1
  local token = docs.token
  local item, selected = selected_item()
  if not item then
    return send_doc(-1, '')
  end
  item_markdown(item, function(markdown)
    vim.schedule(function()
      -- (An answer for an item no longer selected is dropped.)
      if docs.open and token == docs.token then
        send_doc(selected, markdown)
      end
    end)
  end)
end

local function close_doc()
  if docs.timer then
    docs.timer:stop()
  end
  if docs.open then
    docs.open = false
    docs.token = docs.token + 1
    send_doc(-1, '')
  end
end

local function toggle_doc()
  if docs.open then
    return close_doc()
  end
  docs.open = true
  request_doc()
end

-- Keys -----------------------------------------------------------------------

-- What `key` (`<CR>`, `<Tab>`) did before: the first insert mode mapping of it
-- that isn't ours, looked up when the key is pressed, as plugins (mini.pairs,
-- lazy-loaded) map it late, and Neovim's default <Tab> jumps in a snippet.
-- Returns the keys it would type.
local function fallback(key)
  local code = vim.keycode(key)
  for _, map in ipairs(vim.api.nvim_get_keymap('i')) do
    if vim.keycode(map.lhs) == code and not (map.desc or ''):find(DESC, 1, true) then
      if type(map.callback) == 'function' then
        if map.expr ~= 1 then
          -- (Can't change the buffer from an expression.)
          vim.schedule(map.callback)
          return ''
        end
        local keys = map.callback()
        if type(keys) == 'string' and map.replace_keycodes == 1 then
          keys = vim.keycode(keys)
        end
        return keys or ''
      elseif map.rhs then
        local rhs = vim.keycode(map.rhs)
        if map.expr == 1 then
          rhs = vim.api.nvim_eval(rhs)
        end
        return rhs
      end
    end
  end
  return code
end

-- Whether the selection was moved since the last character typed.
local moved = false
local MOVE_KEYS = {}
for _, key in ipairs({ '<C-n>', '<C-p>', '<Up>', '<Down>' }) do
  MOVE_KEYS[vim.keycode(key)] = true
end

-- <CR> accepts the selected item, else does what it did before. The first item
-- is selected by itself, so unless moved to it isn't taken as chosen when nothing
-- was typed for it ("local t = {" would get `false`), and a word typed in full
-- still ends the line ("end", "1").
local function enter()
  if vim.fn.pumvisible() == 0 or vim.fn.complete_info({ 'selected' }).selected < 0 then
    return fallback('<CR>')
  end
  local item = selected_item()
  if item and not moved then
    local before = vim.api.nvim_get_current_line():sub(1, vim.api.nvim_win_get_cursor(0)[2])
    local word = vim.fn.matchstr(before, [[\k\+$]])
    if word == '' then
      return vim.keycode('<C-e>') .. fallback('<CR>')
    elseif word == item.word then
      return vim.keycode('<C-y>') .. fallback('<CR>')
    end
  end
  return vim.keycode('<C-y>')
end

-- <Tab>/<S-Tab> move through the menu; in a snippet they jump (Neovim's default
-- mapping), <C-n>/<C-p> and the arrows still move.
local function tab(key, move, direction)
  return function()
    if vim.fn.pumvisible() == 1 and not vim.snippet.active({ direction = direction }) then
      moved = true
      return vim.keycode(move)
    end
    return fallback(key)
  end
end

-- Buffer-local, to win over the global mapping of a plugin (and of a lazy-loaded
-- one). Not in buffers with a key of their own (a prompt).
local BUFFER_KEYS = {
  { '<CR>', enter, 'accept' },
  { '<Tab>', tab('<Tab>', '<C-n>', 1), 'next item' },
  { '<S-Tab>', tab('<S-Tab>', '<C-p>', -1), 'previous item' },
}
local function map_keys(buf)
  if not vim.api.nvim_buf_is_valid(buf) or vim.bo[buf].buftype ~= '' then
    return
  end
  local taken = {}
  for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, 'i')) do
    taken[vim.keycode(map.lhs)] = true
  end
  for _, k in ipairs(BUFFER_KEYS) do
    if not taken[vim.keycode(k[1])] then
      vim.keymap.set('i', k[1], k[2], { buffer = buf, expr = true, replace_keycodes = false, desc = DESC .. k[3] })
    end
  end
end

-- The command line's arrows: with an item selected they move through the menu.
-- With none, Up recalls history (what's typed is its prefix) and Down enters the
-- menu. (A recalled line opens no menu, so Down steps back through history.)
local function cmdline_arrow(key, move)
  return function()
    local arrow = vim.keycode('<' .. key .. '>')
    local open = vim.fn.wildmenumode() == 1
    if open and vim.fn.cmdcomplete_info().selected >= 0 then
      return vim.keycode(move)
    elseif key == 'Up' then
      return (open and vim.keycode('<C-e>') or '') .. arrow
    end
    return open and vim.keycode(move) or arrow
  end
end

-- Sorting as the menu narrows -------------------------------------------------

-- The language server's list is sorted once, when it answers; typing then only
-- narrows it, in that order. So on each change while the menu is up it's
-- filtered again from the whole list, sorted (what starts with the word typed
-- first) and given back to complete() if the order shown differs. Not after
-- <C-n>, <Tab> and the like: the text they put in isn't typed.
--
-- The whole list and where it starts: from the last complete() call that
-- wasn't ours (Neovim's LSP completion doesn't say).
local whole, whole_start, resorting = nil, nil, false
do
  local complete = vim.fn.complete
  vim.fn.complete = function(col, matches)
    if not resorting then
      whole, whole_start = matches, { vim.api.nvim_win_get_cursor(0)[1], col }
    end
    return complete(col, matches)
  end
end

local function resort()
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  if moved or not whole or whole_start[1] ~= row or whole_start[2] > col + 1 then
    return
  end
  local lsp = false
  for _, item in ipairs(whole) do
    if rank(item) == 0 then
      return -- (Paths start elsewhere than the server's words.)
    end
    lsp = lsp or rank(item) == 1
  end
  if not lsp then
    return
  end
  local typed = vim.api.nvim_get_current_line():sub(whole_start[2], col)
  local list = typed == '' and vim.list_slice(whole) or vim.fn.matchfuzzy(whole, typed, { key = 'word' })
  table.sort(list, lsp_first)
  local function words(items)
    return table.concat(vim.tbl_map(function(item)
      return item.word
    end, items), '\n')
  end
  if #list == 0 or words(list) == words(vim.fn.complete_info({ 'matches' }).matches or {}) then
    return
  end
  resorting = true
  pcall(vim.fn.complete, whole_start[2], list)
  resorting = false
end

-- Setup ------------------------------------------------------------------------

local function setup()
  if vim.g.eshvi_native_completion == false then
    return
  end
  -- (From now on the GUI shows only requested documentation, not items' own.)
  send_doc(-1, '')
  local group = vim.api.nvim_create_augroup('eshvi_completion', { clear = true })
  local function on(events, callback, opts)
    vim.api.nvim_create_autocmd(events, vim.tbl_extend('force', { group = group, callback = callback }, opts or {}))
  end

  -- Insert mode: the menu as you type.
  vim.o.completeopt = 'menuone,noinsert,fuzzy,nosort'
  vim.o.pumheight = MAX_VISIBLE
  vim.o.complete = DEFAULT_SOURCES
  vim.go.autocomplete = true
  -- (Not in pickers' prompts, panels and the like.)
  local function per_buffer(buf)
    if vim.api.nvim_buf_is_valid(buf) then
      vim.bo[buf].autocomplete = vim.bo[buf].buftype == ''
    end
  end
  on({ 'BufEnter', 'FileType', 'BufWinEnter' }, function(a)
    per_buffer(a.buf)
  end)
  on('OptionSet', function(a)
    per_buffer(a.buf)
  end, { pattern = 'buftype' })
  per_buffer(vim.api.nvim_get_current_buf())

  -- After InsertEnter's other handlers: a plugin loading on it (mini.pairs) maps
  -- <CR> only if nothing is mapped yet, and `fallback` needs that mapping.
  on('InsertEnter', function(a)
    vim.schedule(function()
      map_keys(a.buf)
    end)
  end)
  on('BufEnter', function(a)
    if vim.fn.mode() == 'i' then
      map_keys(a.buf)
    end
  end)
  on({ 'InsertCharPre', 'CompleteDone' }, function()
    moved = false
  end)
  vim.on_key(function(key)
    if MOVE_KEYS[key] and vim.fn.mode() == 'i' and vim.fn.pumvisible() == 1 then
      moved = true
    end
  end, vim.api.nvim_create_namespace('eshvi_completion_keys'))

  -- A language server provides items through 'omnifunc' ('o'), and runs
  -- the item's snippet, edits and commands on accepting.
  on('LspAttach', function(a)
    local client = vim.lsp.get_client_by_id(a.data.client_id)
    if not (client and client:supports_method('textDocument/completion', a.buf)) then
      return
    end
    vim.lsp.completion.enable(true, client.id, a.buf, { autotrigger = false, cmp = lsp_first })
    vim.bo[a.buf].complete = LSP_SOURCES
  end)

  -- Documentation, on Ctrl-Space; it follows the selection, then.
  on('CompleteChanged', function()
    if not docs.open then
      return
    end
    docs.timer = docs.timer or vim.uv.new_timer()
    docs.timer:stop()
    docs.timer:start(
      60,
      0,
      vim.schedule_wrap(function()
        if docs.open then
          request_doc()
        end
      end)
    )
  end)
  on({ 'CompleteDone', 'InsertLeave' }, close_doc)
  on('TextChangedP', resort)
  -- Ctrl-Space: the menu showing: its docs, on or off; else ask the
  -- 'omnifunc' (the language server) for items. Not if the config has its own.
  if vim.fn.maparg('<C-Space>', 'i') == '' then
    vim.keymap.set('i', '<C-Space>', function()
      if vim.fn.pumvisible() == 1 then
        toggle_doc()
      elseif vim.bo.omnifunc ~= '' then
        vim.api.nvim_feedkeys(vim.keycode('<C-x><C-o>'), 'n', false)
      end
    end, { desc = DESC .. 'documentation' })
  end

  -- The command line: the menu as you type; <Tab>/<S-Tab> move, built in.
  vim.o.wildmode = 'noselect:lastused,full'
  vim.o.wildoptions = 'pum'
  on('CmdlineChanged', function()
    if vim.fn.getcmdtype():find('^[:/?]$') then
      vim.fn.wildtrigger()
    end
  end)
  vim.keymap.set('c', '<Up>', cmdline_arrow('Up', '<C-p>'), { expr = true, replace_keycodes = false, desc = DESC .. 'menu or history' })
  vim.keymap.set('c', '<Down>', cmdline_arrow('Down', '<C-n>'), { expr = true, replace_keycodes = false, desc = DESC .. 'menu or history' })
end

if vim.v.vim_did_enter == 1 then
  setup()
else
  vim.api.nvim_create_autocmd('VimEnter', { once = true, callback = setup })
end
