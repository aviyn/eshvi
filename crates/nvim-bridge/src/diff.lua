-- Neovim's diffs of a file's or a commit's changes, for the Source Control
-- panel and the agents' Changes popup, when `[git] diff` / `[agent] diff` say
-- "tab" or "buffer" (the default, "native", is the GUI's own view: no call
-- comes here). Loaded whether or not the agent panel is.
-- Argument: the GUI's channel.
local chan = ...
_G.Eshvi = _G.Eshvi or {}

-- The GUI removed `full` (`rel` in the repository): the agents created it
-- and every line of it was undone. Its buffer, and its diff's "(original)"
-- side, go too (left open, Neovim would warn that the file is gone).
function Eshvi.changes_removed(full, rel)
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(b)
    local file = vim.fn.resolve(name) == vim.fn.resolve(full) and not vim.bo[b].modified
    if file or name:sub(-#(rel .. ' (original)')) == rel .. ' (original)' then
      pcall(vim.api.nvim_buf_delete, b, {})
    end
  end
  vim.notify(('%s: removed (the agents created it; nothing of it is left)'):format(rel))
end

-- The GUI's diff view asks for these (the agents' changes, "revert"; Source
-- Control's "discard"). Each answers with `gui_diff_edit`.
local function edit_done(op, full, ok, message)
  if message and message ~= '' and not ok then
    vim.notify(message, vim.log.levels.WARN)
  end
  vim.rpcnotify(chan, 'gui_diff_edit', { op = op, path = full, ok = ok, message = message or '' })
end

local function lines_equal(a, b)
  if #a ~= #b then
    return false
  end
  for i = 1, #a do
    if a[i] ~= b[i] then
      return false
    end
  end
  return true
end

-- The buffer of `full` if it is loaded.
local function loaded_buf(full)
  local want = vim.fn.resolve(full)
  for _, b in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(b) and vim.fn.resolve(vim.api.nvim_buf_get_name(b)) == want then
      return b
    end
  end
end

-- Put a change back: in `full`'s buffer, lines `start` (0-based) .. +#expect, which
-- must be `expect` (as the view read them), become `put`, and the file is
-- written (as `do` and `:w` did in the diff of old), without changing windows,
-- so that an open buffer, the undo history and the GUI's tracking stay right.
-- A buffer with unsaved changes of the user's is left alone.
function Eshvi.diff_revert(full, start, expect, put)
  local name = vim.fn.fnamemodify(full, ':t')
  local buf = vim.fn.bufadd(full)
  vim.fn.bufload(buf)
  if vim.bo[buf].modified then
    return edit_done('revert', full, false, ('%s has unsaved changes in Neovim: save or undo them first'):format(name))
  end
  local gone = vim.fn.filereadable(full) == 0
  if not gone then
    -- (A file changed on disk since Neovim read it is read again.)
    pcall(vim.cmd, 'silent! checktime ' .. buf)
  end
  local last = start + #expect
  if gone then
    -- The agents deleted it: what the buffer holds is not the file.
    start, last = 0, -1
  else
    local now = vim.api.nvim_buf_get_lines(buf, start, last, false)
    if not lines_equal(now, expect) then
      return edit_done('revert', full, false, ('%s changed since the diff was read: look again'):format(name))
    end
    -- (A file with no lines has one empty one for Neovim: replace it.)
    if #expect == 0 and start == 0 and vim.fn.getfsize(full) <= 0 then
      last = -1
    end
  end
  vim.fn.mkdir(vim.fn.fnamemodify(full, ':h'), 'p')
  local ok, err = pcall(function()
    vim.api.nvim_buf_set_lines(buf, start, last, false, put)
    vim.api.nvim_buf_call(buf, function()
      vim.cmd('silent write')
    end)
  end)
  edit_done('revert', full, ok, ok and '' or ('%s: %s'):format(name, tostring(err)))
end

-- Whether none of `paths` has unsaved changes in Neovim (a discard would
-- take the file from under them).
function Eshvi.diff_guard(paths)
  for _, full in ipairs(paths) do
    local b = loaded_buf(full)
    if b and vim.bo[b].modified then
      return edit_done('guard', full, false, ('%s has unsaved changes in Neovim: save or undo them first'):format(vim.fn.fnamemodify(full, ':t')))
    end
  end
  edit_done('guard', paths[1] or '', true, '')
end

local changes_diff_tab, unified, diff_buffer

-- A count of the calls, so that a slow `git show` of an earlier one doesn't
-- open after a later one.
local diff_calls = 0

-- A read-only buffer with `text` (the output of `git show`, or nothing),
-- in `full`'s filetype.
local function scratch_buf(name, text, full)
  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = 'wipe'
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.split((text:gsub('\n$', '')), '\n', { plain = true }))
  vim.bo[buf].modifiable = false
  pcall(vim.api.nvim_buf_set_name, buf, name)
  local ft = vim.filetype.match({ filename = full })
  if ft then
    vim.bo[buf].filetype = ft
  end
  return buf
end

-- `ref:path` of the repository `top`, or nothing if it has none (a new
-- file, no commit yet), to `done`.
local function git_show(top, ref, path, done)
  vim.system({ 'git', '-C', top, 'show', ref .. ':' .. path }, { text = true }, vim.schedule_wrap(function(r)
    done(r.code == 0 and r.stdout or '')
  end))
end

-- The file as it is on disk (nothing if it's gone), as `git show` reads.
local function read_file(full)
  local f = io.open(full, 'rb')
  if not f then
    return ''
  end
  local text = f:read('*a')
  f:close()
  return (text:gsub('\r\n', '\n'))
end

-- A file's changes since the GUI's snapshot (`base` in repository `top`),
-- from the GUI's Changes popup: the snapshot's version left of the file, as
-- a diff. `how` (`[agent] diff`, `[git] diff`): "buffer", as `git diff`
-- prints it, in a buffer in an editor window (q goes back); anything else, a
-- tab page of its own with both sides in diff mode. A deleted file shows its
-- last version alone.
--
-- `opts`, for the Source Control panel, all optional: `left_path` (the left
-- side's path, if it was renamed), `right` (a ref, "" the index: a
-- read-only right side from `git show`, instead of the file), `left_label`
-- and `right_label` (the buffers' names).
function Eshvi.changes_diff(top, base, path, deleted, how, opts)
  opts = opts or {}
  local full = vim.fs.joinpath(top, path)
  diff_calls = diff_calls + 1
  local call = diff_calls
  local before, after
  local function open()
    if call ~= diff_calls then
      return
    end
    if how == 'buffer' then
      local now = after or (deleted and '') or read_file(full)
      if before == now then
        vim.notify(('%s: no changes'):format(path))
      else
        local name = opts.left_label and (' (%s → %s)'):format(opts.left_label, opts.right_label or 'file') or ' (changes)'
        diff_buffer(path .. name, unified(opts.left_path or path, path, before, now))
      end
      return
    end
    local buf = scratch_buf(path .. ' (' .. (opts.left_label or 'original') .. ')', before, full)
    local file_buf
    if deleted then
      file_buf = nil
    elseif opts.right then
      file_buf = scratch_buf(path .. ' (' .. (opts.right_label or (opts.right ~= '' and opts.right or 'index')) .. ')', after, full)
    else
      file_buf = vim.fn.bufadd(full)
      vim.fn.bufload(file_buf)
      vim.bo[file_buf].buflisted = true
      if opts.left_label == nil and not vim.b[file_buf].eshvi_diff_saves then
        -- A save with the file's diff on screen is the user's "undo" ending
        -- (`do` on every hunk, then :w): the GUI may then remove a file the
        -- agents created. Any other save of it is not that.
        vim.b[file_buf].eshvi_diff_saves = true
        vim.api.nvim_create_autocmd('BufWritePost', {
          buffer = file_buf,
          callback = function(a)
            for _, w in ipairs(vim.fn.win_findbuf(a.buf)) do
              if vim.wo[w].diff then
                vim.rpcnotify(chan, 'gui_diff_saved', vim.fn.fnamemodify(a.match, ':p'))
                return
              end
            end
          end,
        })
      end
    end
    local d = {
      path = path,
      read_only = opts.right ~= nil,
      agents = opts.left_label == nil,
    }
    changes_diff_tab(d, buf, file_buf)
  end
  local waiting = (opts.right ~= nil and not deleted) and 2 or 1
  local function got()
    waiting = waiting - 1
    if waiting == 0 then
      open()
    end
  end
  git_show(top, base, opts.left_path or path, function(text)
    before = text
    got()
  end)
  if waiting == 2 then
    git_show(top, opts.right, path, function(text)
      after = text
      got()
    end)
  end
end

local function diff_on(wins)
  if #wins == 2 then
    for _, w in ipairs(wins) do
      vim.api.nvim_win_call(w, function()
        vim.cmd('diffthis')
      end)
    end
  end
end

-- The diff in a tab page of its own: closing the tab (or the original's
-- window, or q there) closes both sides.
changes_diff_tab = function(d, buf, file_buf)
  local path = d.path
  vim.cmd('tabnew')
  local tab = vim.api.nvim_get_current_tabpage()
  local empty = vim.api.nvim_get_current_buf()
  local sides = {}
  if file_buf then
    vim.api.nvim_win_set_buf(0, file_buf)
    sides[2] = vim.api.nvim_get_current_win()
    vim.cmd('leftabove vsplit')
  end
  vim.api.nvim_win_set_buf(0, buf)
  sides[1] = vim.api.nvim_get_current_win()
  if vim.api.nvim_buf_is_valid(empty) and empty ~= file_buf then
    pcall(vim.api.nvim_buf_delete, empty, {})
  end
  -- A header like the file's beside it (the layout's winbar, set once the
  -- new windows are reported, is only for file buffers): the GUI draws it
  -- as a tab, and the two sides line up.
  vim.schedule(function()
    local file_win = sides[2] and vim.api.nvim_win_is_valid(sides[2]) and sides[2]
    if vim.api.nvim_win_is_valid(sides[1]) and (vim.o.winbar ~= '' or (file_win and vim.wo[file_win].winbar ~= '')) then
      vim.wo[sides[1]][0].winbar = '%t'
    end
  end)
  diff_on(sides)
  local function close()
    if vim.api.nvim_tabpage_is_valid(tab) and #vim.api.nvim_list_tabpages() > 1 then
      vim.cmd('tabclose ' .. vim.api.nvim_tabpage_get_number(tab))
    end
  end
  vim.keymap.set('n', 'q', close, { buffer = buf, nowait = true, desc = 'eshvi: close the diff' })
  if d.read_only and file_buf then
    vim.keymap.set('n', 'q', close, { buffer = file_buf, nowait = true, desc = 'eshvi: close the diff' })
  end
  vim.api.nvim_create_autocmd('BufWipeout', { buffer = buf, once = true, callback = vim.schedule_wrap(close) })
  if file_buf then
    vim.api.nvim_set_current_win(sides[2])
  end
  if file_buf and d.agents then
    vim.notify(('%s: changes since the agents started. To undo one, `do` on it here, then :w'):format(path))
  end
end

-- Two versions of a file as `git diff` prints them (the versions are what
-- the other views show, so a new file, a first commit or a file only the
-- agents' snapshot has need no case of their own).
unified = function(left, right, before, after)
  local old = before == '' and '/dev/null' or ('a/' .. left)
  local new = after == '' and '/dev/null' or ('b/' .. right)
  local head = ('diff --git a/%s b/%s\n'):format(left, right)
  if before:find('\0', 1, true) or after:find('\0', 1, true) then
    return head .. ('Binary files %s and %s differ\n'):format(old, new)
  end
  local diff = (vim.text and vim.text.diff) or vim.diff
  return head .. ('--- %s\n+++ %s\n'):format(old, new) .. diff(before, after, { ctxlen = 3 })
end

-- The window a diff buffer goes in: this one if it shows a file or another
-- diff, else the editor's; with no editor window, a new one.
local function diff_window()
  local function usable(w)
    local b = vim.api.nvim_win_get_buf(w)
    return vim.api.nvim_win_get_config(w).relative == '' and (vim.bo[b].buftype == '' or vim.b[b].eshvi_diff_prev ~= nil)
  end
  if usable(vim.api.nvim_get_current_win()) then
    return vim.api.nvim_get_current_win()
  end
  local editor = Eshvi.editor_window()
  if editor then
    return editor
  end
  vim.cmd('leftabove vsplit')
  return vim.api.nvim_get_current_win()
end

-- `text` in a read-only buffer of filetype `diff` in an editor window. q goes
-- back to what the window showed (from before the first, when one diff
-- replaces another).
diff_buffer = function(name, text)
  local win = diff_window()
  vim.api.nvim_set_current_win(win)
  local was = vim.api.nvim_win_get_buf(win)
  -- ('nohidden' won't let unsaved changes leave their window.)
  if vim.bo[was].modified and not vim.o.hidden then
    vim.cmd('split')
    win = vim.api.nvim_get_current_win()
  end
  local prev = vim.b[was].eshvi_diff_prev or was
  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = 'wipe'
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.split((text:gsub('\n$', '')), '\n', { plain = true }))
  vim.bo[buf].modifiable = false
  vim.b[buf].eshvi_diff_prev = prev
  vim.keymap.set('n', 'q', function()
    Eshvi.close_diff(0)
  end, { buffer = buf, nowait = true, desc = 'eshvi: close the diff' })
  vim.api.nvim_win_set_buf(win, buf)
  -- (Named here: the diff it replaces, perhaps of this name, is gone now.)
  pcall(vim.api.nvim_buf_set_name, buf, name)
  vim.bo[buf].filetype = 'diff'
end

-- All of a commit's changes, from the Source Control panel's graph: `git show`
-- in a diff buffer, a merge against its first parent (as its files' diffs are).
function Eshvi.commit_diff(top, oid)
  diff_calls = diff_calls + 1
  local call = diff_calls
  local cmd = { 'git', '-C', top, 'show', '--stat', '--patch', '--first-parent', '--no-color', '--no-ext-diff', oid }
  vim.system(cmd, { text = true }, vim.schedule_wrap(function(r)
    if call ~= diff_calls then
      return
    elseif r.code ~= 0 then
      vim.notify(('git show %s: %s'):format(oid, r.stderr), vim.log.levels.ERROR)
    else
      diff_buffer(oid:sub(1, 7) .. ' (commit)', r.stdout)
    end
  end))
end

-- Open `path` at `line` (1-based) in an editor window, as the explorer opens
-- a file (`open_file.lua`): for the GUI's diff view, which closes itself.
function Eshvi.open_at(path, line)
  local function usable(w)
    local buf = vim.api.nvim_win_get_buf(w)
    return vim.api.nvim_win_get_config(w).relative == '' and vim.bo[buf].buftype == ''
  end
  local editor = Eshvi.editor_window()
  if editor and not usable(vim.api.nvim_get_current_win()) then
    vim.api.nvim_set_current_win(editor)
  end
  vim.cmd('drop ' .. vim.fn.fnameescape(path))
  local last = vim.api.nvim_buf_line_count(0)
  pcall(vim.api.nvim_win_set_cursor, 0, { math.max(1, math.min(line, last)), 0 })
  vim.cmd('normal! zz')
end
