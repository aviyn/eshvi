-- Claude Code's IDE connection, Neovim's side (the server is ide.rs):
-- reports the current file and selection, sends files or lines as context
-- (:EshviAgentSend), and answers the tools Claude calls (Eshvi.ide.*).
-- Argument: the GUI's channel.
local chan = ...
_G.Eshvi = _G.Eshvi or {}
local M = {}
Eshvi.ide = M

-- A file being edited (not a terminal, list or scratch buffer).
local function is_file(buf)
  return vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].buftype == '' and vim.api.nvim_buf_get_name(buf) ~= ''
end

local function path_of(buf)
  return vim.fs.normalize(vim.api.nvim_buf_get_name(buf))
end

-- The buffer showing `path`, if loaded.
local function buf_of(path)
  path = vim.fs.normalize(path)
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and path_of(b) == path then
      return b
    end
  end
end

-- Selection --------------------------------------------------------------

-- The selection in the current window, as Claude Code takes it: 0-based
-- positions; without a visual selection, the cursor (an empty selection).
local function selection()
  local buf = vim.api.nvim_get_current_buf()
  -- A Markdown preview beside its file (a scratch window): the file it
  -- shows, at the line of the window showing that file.
  local of = vim.w.eshvi_preview_of
  if of and is_file(of) then
    local line = 0
    for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
      if vim.api.nvim_win_get_buf(w) == of and not vim.w[w].eshvi_preview_of then
        line = vim.api.nvim_win_get_cursor(w)[1] - 1
        break
      end
    end
    local path = path_of(of)
    local pos = { line = line, character = 0 }
    return {
      filePath = path,
      fileUrl = vim.uri_from_fname(path),
      text = '',
      selection = { start = pos, ['end'] = pos, isEmpty = true },
    }
  end
  if not is_file(buf) then
    return nil
  end
  local path = path_of(buf)
  local mode = vim.fn.mode()
  local cur = vim.api.nvim_win_get_cursor(0)
  local params = { filePath = path, fileUrl = vim.uri_from_fname(path) }
  if mode == 'v' or mode == 'V' or mode == '\22' then
    local v = vim.fn.getpos('v')
    local a, b = { v[2], v[3] }, { cur[1], cur[2] + 1 }
    if a[1] > b[1] or (a[1] == b[1] and a[2] > b[2]) then
      a, b = b, a
    end
    local lines
    if mode == 'V' then
      lines = vim.api.nvim_buf_get_lines(buf, a[1] - 1, b[1], false)
      a[2], b[2] = 1, #(lines[#lines] or '') + 1
    else
      lines = vim.fn.getregion(vim.fn.getpos('v'), vim.fn.getpos('.'), { type = mode })
    end
    params.text = table.concat(lines, '\n')
    params.selection = {
      start = { line = a[1] - 1, character = a[2] - 1 },
      ['end'] = { line = b[1] - 1, character = b[2] - (mode == 'V' and 1 or 0) },
      isEmpty = false,
    }
  else
    params.text = ''
    local pos = { line = cur[1] - 1, character = cur[2] }
    params.selection = { start = pos, ['end'] = pos, isEmpty = true }
  end
  return params
end

local last_sent, timer = nil, nil
local function send_selection()
  local params = selection()
  -- Not a file (a terminal, a list): Claude keeps the last
  -- file's selection, so it is still there when typing to it.
  if not params then
    return
  end
  local key = vim.inspect(params)
  if key ~= last_sent then
    last_sent = key
    vim.rpcnotify(chan, 'eshvi_ide_selection', params)
  end
end

local group = vim.api.nvim_create_augroup('EshviIde', { clear = true })
vim.api.nvim_create_autocmd({ 'CursorMoved', 'CursorMovedI', 'ModeChanged', 'BufEnter', 'WinEnter' }, {
  group = group,
  callback = function()
    -- Debounced: a selection being dragged sends once it rests.
    timer = timer or vim.uv.new_timer()
    timer:stop()
    timer:start(80, 0, vim.schedule_wrap(send_selection))
  end,
})

-- Sending context ---------------------------------------------------------

-- Send `path` (lines `l1`..`l2`, 1-based, if given) to the shown agent, as
-- if @-mentioned in its prompt, and focus it.
function M.send(path, l1, l2)
  path = vim.fs.normalize(vim.fn.fnamemodify(path, ':p'))
  local params = { filePath = path, pid = Eshvi.agent_pid and Eshvi.agent_pid() or nil }
  if l1 then
    params.lineStart = l1 - 1
    params.lineEnd = (l2 or l1) - 1
  end
  vim.rpcnotify(chan, 'eshvi_ide_mention', params)
  if vim.fn.exists(':EshviAgent') == 2 then
    vim.cmd('EshviAgent focus')
  end
end

-- :EshviAgentSend [path]: the file (lines, with a range) to the agent.
vim.api.nvim_create_user_command('EshviAgentSend', function(o)
  local path = o.args ~= '' and o.args or nil
  if not path then
    local buf = vim.api.nvim_get_current_buf()
    if not is_file(buf) then
      return vim.notify('EshviAgentSend: not a file', vim.log.levels.WARN)
    end
    path = path_of(buf)
  end
  if o.range > 0 and not o.args:find('%S') then
    M.send(path, o.line1, o.line2)
  else
    M.send(path)
  end
end, { nargs = '?', range = true, complete = 'file', desc = 'eshvi: send the file (or lines) to the agent' })

-- Tools -------------------------------------------------------------------

local severity = { 'Error', 'Warning', 'Information', 'Hint' }

local function diagnostics_of(buf)
  local list = {}
  for _, d in ipairs(vim.diagnostic.get(buf)) do
    table.insert(list, {
      message = d.message,
      severity = severity[d.severity] or 'Error',
      source = d.source,
      code = d.code and tostring(d.code) or nil,
      range = {
        start = { line = d.lnum, character = d.col },
        ['end'] = { line = d.end_lnum or d.lnum, character = d.end_col or d.col },
      },
    })
  end
  return list
end

-- getDiagnostics {uri?}: one file's, or every file's that has some.
function M.diagnostics(args)
  local uri = type(args) == 'table' and args.uri or nil
  local out = {}
  if uri and uri ~= '' then
    local buf = buf_of(vim.uri_to_fname(uri))
    table.insert(out, { uri = uri, diagnostics = buf and diagnostics_of(buf) or {} })
    return out
  end
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if is_file(buf) then
      local list = diagnostics_of(buf)
      if #list > 0 then
        table.insert(out, { uri = vim.uri_from_fname(path_of(buf)), diagnostics = list })
      end
    end
  end
  return out
end

function M.open_editors()
  local tabs, cur = {}, vim.api.nvim_get_current_buf()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if is_file(buf) and vim.bo[buf].buflisted then
      local path = path_of(buf)
      table.insert(tabs, {
        uri = vim.uri_from_fname(path),
        isActive = buf == cur,
        label = vim.fs.basename(path),
        languageId = vim.bo[buf].filetype,
        isDirty = vim.bo[buf].modified,
      })
    end
  end
  return { tabs = tabs }
end

function M.document_dirty(args)
  local buf = buf_of(args.filePath)
  if not buf then
    return { success = false, message = 'Document not open: ' .. args.filePath }
  end
  return { success = true, filePath = args.filePath, isDirty = vim.bo[buf].modified, isUntitled = false }
end

function M.save_document(args)
  local buf = buf_of(args.filePath)
  if not buf then
    return { success = false, message = 'Document not open: ' .. args.filePath }
  end
  local ok, err = pcall(vim.api.nvim_buf_call, buf, function()
    vim.cmd('silent write')
  end)
  return { success = ok, filePath = args.filePath, saved = ok, message = ok and 'Saved' or tostring(err) }
end

function M.open_file(args)
  local ok = Eshvi.agent_open and Eshvi.agent_open(args.filePath)
  if ok and args.startText and args.startText ~= '' then
    vim.fn.search(vim.fn.escape(args.startText, '\\/.*$^~[]'), 'cw')
  end
  return { success = ok == true, filePath = args.filePath }
end

-- Diff view ---------------------------------------------------------------
-- Claude's proposed edit (openDiff) beside the file, as a Neovim diff in
-- the editor area (the panels stay). `:w` in the proposal accepts (the
-- file is written; Claude gets the final text, the user's own changes to
-- the proposal included), `:q` rejects; so do :EshviDiffAccept and
-- :EshviDiffReject. Answered in Claude's own prompt instead, Claude closes
-- it (close_tab). Each view answers once (eshvi_ide_diff).

-- By tab name: { path, buf (proposal), win, orig_win, eol, answered, id }.
-- `id` is the openDiff call that opened the view: the answer goes to that
-- call, so a second call with the same tab name (which closes the first
-- view) isn't answered by the first one's rejection.
local diffs = {}

local function answer(tab, content)
  local d = diffs[tab]
  if not d or d.answered then
    return
  end
  d.answered = true
  vim.rpcnotify(chan, 'eshvi_ide_diff', { tab = tab, id = d.id, content = content })
end

local function text(...)
  local out = {}
  for _, s in ipairs({ ... }) do
    table.insert(out, { type = 'text', text = s })
  end
  return out
end

-- Close a diff view, leaving the file's window as it was.
local function cleanup(tab)
  local d = diffs[tab]
  if not d then
    return
  end
  diffs[tab] = nil
  if d.orig_win and vim.api.nvim_win_is_valid(d.orig_win) then
    vim.api.nvim_win_call(d.orig_win, function()
      vim.cmd('diffoff')
    end)
  end
  if vim.api.nvim_win_is_valid(d.win) then
    pcall(vim.api.nvim_win_close, d.win, true)
  end
  if vim.api.nvim_buf_is_valid(d.buf) then
    pcall(vim.api.nvim_buf_delete, d.buf, { force = true })
  end
  if d.scratch and vim.api.nvim_buf_is_valid(d.scratch) then
    pcall(vim.api.nvim_buf_delete, d.scratch, { force = true })
  end
end

-- The diff the cursor is in (the proposal or its file), else the newest.
local function current_diff()
  local buf, win = vim.api.nvim_get_current_buf(), vim.api.nvim_get_current_win()
  local newest
  for tab, d in pairs(diffs) do
    if d.buf == buf or d.orig_win == win then
      return tab
    end
    if not newest or d.seq > diffs[newest].seq then
      newest = tab
    end
  end
  return newest
end

function M.accept_diff(tab)
  tab = tab or current_diff()
  local d = tab and diffs[tab]
  if not d then
    return
  end
  local lines = vim.api.nvim_buf_get_lines(d.buf, 0, -1, false)
  vim.fn.mkdir(vim.fs.dirname(d.path), 'p')
  vim.fn.writefile(lines, d.path, d.eol and '' or 'b')
  -- Buffers of the file read it again (unless the user changed them).
  local b = buf_of(d.path)
  if b and not vim.bo[b].modified then
    vim.api.nvim_buf_call(b, function()
      vim.cmd('silent! edit!')
    end)
  end
  local content = table.concat(lines, '\n') .. (d.eol and '\n' or '')
  answer(tab, text('FILE_SAVED', content))
  cleanup(tab)
end

function M.reject_diff(tab)
  tab = tab or current_diff()
  if not tab then
    return
  end
  answer(tab, text('DIFF_REJECTED', tab))
  cleanup(tab)
end

-- openDiff {old_file_path, new_file_path, new_file_contents, tab_name}.
local seq = 0
function M.open_diff(args)
  local tab = args.tab_name or 'diff'
  if diffs[tab] then
    M.reject_diff(tab)
  end
  local path = vim.fs.normalize(args.new_file_path or args.old_file_path)
  local new = args.new_file_contents or ''
  local eol = new:sub(-1) == '\n'
  local lines = vim.split(eol and new:sub(1, -2) or new, '\n', { plain = true })

  -- The file, in an editor window (one showing it, else the one last used).
  local orig_win
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    local b = vim.api.nvim_win_get_buf(w)
    if vim.api.nvim_win_get_config(w).relative == '' and is_file(b) and path_of(b) == path then
      orig_win = w
    end
  end
  local scratch
  if not orig_win then
    local exists = vim.fn.filereadable(path) == 1
    if exists and Eshvi.agent_open and Eshvi.agent_open(path) then
      orig_win = vim.api.nvim_get_current_win()
    else
      -- A new file: an empty "before" side.
      for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
        if vim.api.nvim_win_get_config(w).relative == '' and is_file(vim.api.nvim_win_get_buf(w)) then
          orig_win = w
          break
        end
      end
      orig_win = orig_win or vim.api.nvim_get_current_win()
      vim.api.nvim_set_current_win(orig_win)
      vim.cmd('enew')
      scratch = vim.api.nvim_get_current_buf()
      vim.bo[scratch].buftype = 'nofile'
      vim.bo[scratch].bufhidden = 'wipe'
      pcall(vim.api.nvim_buf_set_name, scratch, path .. ' (new file)')
      orig_win = vim.api.nvim_get_current_win()
    end
  end
  vim.api.nvim_set_current_win(orig_win)

  -- The proposal, right of it.
  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].buftype = 'acwrite'
  vim.bo[buf].bufhidden = 'wipe'
  vim.bo[buf].swapfile = false
  pcall(vim.api.nvim_buf_set_name, buf, ('%s (proposed by the agent: :w accepts, :q rejects)'):format(vim.fs.basename(path)))
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modified = false
  local ft = vim.filetype.match({ filename = path })
  if ft then
    vim.bo[buf].filetype = ft
  end
  vim.cmd('rightbelow vsplit')
  local win = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_buf(win, buf)
  for _, w in ipairs({ orig_win, win }) do
    vim.api.nvim_win_call(w, function()
      vim.cmd('diffthis')
    end)
  end
  seq = seq + 1
  diffs[tab] = { path = path, buf = buf, win = win, orig_win = orig_win, eol = eol, seq = seq, scratch = scratch, id = args.call_id }

  vim.api.nvim_create_autocmd('BufWriteCmd', {
    buffer = buf,
    callback = function()
      M.accept_diff(tab)
    end,
  })
  -- Closed without :w: rejected.
  vim.api.nvim_create_autocmd({ 'BufWinLeave', 'BufWipeout' }, {
    buffer = buf,
    once = true,
    callback = function()
      vim.schedule(function()
        -- (Still this view: a later one under the same tab name is not
        -- rejected because this one closed.)
        if diffs[tab] and diffs[tab].buf == buf then
          M.reject_diff(tab)
        end
      end)
    end,
  })
  vim.notify(('The agent proposes changes to %s: :w accepts, :q rejects'):format(vim.fs.basename(path)))
  return true
end

-- close_tab {tab_name}: Claude got its answer elsewhere (its own prompt).
function M.close_diff(tab)
  answer(tab, text('TAB_CLOSED'))
  cleanup(tab)
  return true
end

function M.close_all_diffs()
  local n = 0
  for tab in pairs(diffs) do
    M.close_diff(tab)
    n = n + 1
  end
  return n
end

vim.api.nvim_create_user_command('EshviDiffAccept', function()
  M.accept_diff()
end, { desc = "eshvi: accept the agent's proposed edit" })
vim.api.nvim_create_user_command('EshviDiffReject', function()
  M.reject_diff()
end, { desc = "eshvi: reject the agent's proposed edit" })
