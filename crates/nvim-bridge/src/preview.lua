-- eshvi: Markdown preview. The GUI draws a Markdown buffer rendered over a
-- window: the file's own (in place, `w:eshvi_preview`: the window keeps the
-- buffer and its cursor, which the preview follows) or a scratch window
-- beside it (`w:eshvi_preview_of`, the file's buffer). Neovim keeps the
-- windows (splits, :q, <C-w>), the GUI only draws.
--   :EshviPreview [toggle]  this window in place
--   :EshviPreview side      a preview beside it (again: closes it)
--   :EshviPreview off       this window back to its text
-- Reports: gui_preview {wins = {{win, buf, line, side, path}}} (this tab
-- page's previews: the source buffer, its file, the line to show), gui_preview_text
-- {buf, text} (a previewed buffer's text, again as it changes).
local chan = ...

local group = vim.api.nvim_create_augroup('eshvi_preview', { clear = true })

local function is_markdown(buf)
  return vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].filetype == 'markdown'
end

-- The buffer a window previews, and whether beside it.
local function previewed(win)
  local of = vim.w[win].eshvi_preview_of
  if of then
    return of, true
  end
  if vim.w[win].eshvi_preview then
    return vim.api.nvim_win_get_buf(win), false
  end
end

-- The line a preview shows: in place, its window's cursor line; beside,
-- the cursor line of the window showing the source (the current one if it
-- does, else the last one left).
local function source_line(buf)
  local cur = vim.api.nvim_get_current_win()
  if vim.api.nvim_win_get_buf(cur) == buf then
    return vim.api.nvim_win_get_cursor(cur)[1]
  end
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(w) == buf and not vim.w[w].eshvi_preview_of then
      return vim.api.nvim_win_get_cursor(w)[1]
    end
  end
  return 1
end

-- Text sent, by buffer: changedtick then, so it goes again only when it
-- changed (and once per buffer, whichever windows show it).
local sent_tick = {}
local function send_text(buf)
  local tick = vim.api.nvim_buf_get_changedtick(buf)
  if sent_tick[buf] == tick then
    return
  end
  sent_tick[buf] = tick
  local text = table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), '\n')
  vim.rpcnotify(chan, 'gui_preview_text', { buf = buf, text = text })
end

local last_report = nil
local function report()
  local wins = {}
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    local buf, side = previewed(w)
    if buf and vim.api.nvim_buf_is_valid(buf) then
      local line = side and source_line(buf) or vim.api.nvim_win_get_cursor(w)[1]
      local path = vim.api.nvim_buf_get_name(buf)
      table.insert(wins, { win = w, buf = buf, line = line, side = side, path = path })
      send_text(buf)
    end
  end
  local key = vim.inspect(wins)
  if key ~= last_report then
    last_report = key
    vim.rpcnotify(chan, 'gui_preview', { wins = wins })
  end
end

local function any()
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if previewed(w) then
      return true
    end
  end
  return false
end

-- Side previews of `buf` in this tab page.
local function side_wins(buf)
  local found = {}
  for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.w[w].eshvi_preview_of == buf then
      table.insert(found, w)
    end
  end
  return found
end

local function in_place(win, on)
  vim.w[win].eshvi_preview = on or nil
  report()
end

local function side()
  local src = vim.api.nvim_get_current_buf()
  local existing = side_wins(src)
  if #existing > 0 then
    for _, w in ipairs(existing) do
      pcall(vim.api.nvim_win_close, w, true)
    end
    return report()
  end
  local from = vim.api.nvim_get_current_win()
  vim.w[from].eshvi_preview = nil
  vim.cmd('vertical rightbelow new')
  local win, buf = vim.api.nvim_get_current_win(), vim.api.nvim_get_current_buf()
  vim.bo[buf].buftype = 'nofile'
  vim.bo[buf].bufhidden = 'wipe'
  vim.bo[buf].swapfile = false
  vim.bo[buf].modifiable = false
  vim.bo[buf].filetype = 'eshvi_preview'
  vim.w[win].eshvi_preview_of = src
  vim.wo[win].winbar = 'Preview ' .. vim.fn.fnamemodify(vim.api.nvim_buf_get_name(src), ':t')
  for _, opt in ipairs({ 'number', 'relativenumber', 'cursorline', 'list', 'spell' }) do
    vim.wo[win][opt] = false
  end
  vim.wo[win].signcolumn = 'no'
  vim.wo[win].foldcolumn = '0'
  vim.api.nvim_set_current_win(from)
  report()
end

local actions = {
  toggle = function()
    local win = vim.api.nvim_get_current_win()
    if vim.w[win].eshvi_preview_of then
      -- A side preview: close it.
      return vim.api.nvim_win_close(win, true)
    end
    if not is_markdown(vim.api.nvim_get_current_buf()) then
      return vim.notify('EshviPreview: not a Markdown file', vim.log.levels.WARN)
    end
    in_place(win, not vim.w[win].eshvi_preview)
  end,
  side = function()
    if not is_markdown(vim.api.nvim_get_current_buf()) then
      return vim.notify('EshviPreview: not a Markdown file', vim.log.levels.WARN)
    end
    side()
  end,
  off = function()
    in_place(vim.api.nvim_get_current_win(), false)
  end,
}

vim.api.nvim_create_user_command('EshviPreview', function(o)
  local action = actions[o.fargs[1] or 'toggle']
  if not action then
    return vim.notify('EshviPreview: unknown action ' .. o.args, vim.log.levels.ERROR)
  end
  action()
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'side', 'off' }
  end,
  desc = 'eshvi: Markdown preview (in place, or beside)',
})

-- In place: editing goes back to the text (never typing into a hidden
-- buffer); so does a buffer in the window that isn't Markdown (a link to
-- another Markdown file stays in the preview, as VS Code's does).
vim.api.nvim_create_autocmd('ModeChanged', {
  group = group,
  pattern = '*:[iRs]*',
  callback = function()
    if vim.w.eshvi_preview then
      in_place(vim.api.nvim_get_current_win(), false)
    end
  end,
})
vim.api.nvim_create_autocmd('BufWinEnter', {
  group = group,
  callback = function()
    local win = vim.api.nvim_get_current_win()
    if vim.w[win].eshvi_preview and not is_markdown(vim.api.nvim_win_get_buf(win)) then
      in_place(win, false)
    end
  end,
})
-- A file gone: its side previews go too.
vim.api.nvim_create_autocmd({ 'BufWipeout', 'BufDelete' }, {
  group = group,
  callback = function(ev)
    sent_tick[ev.buf] = nil
    for _, w in ipairs(side_wins(ev.buf)) do
      vim.schedule(function()
        pcall(vim.api.nvim_win_close, w, true)
      end)
    end
  end,
})

-- Following the text and the cursor (only while there is a preview).
local pending = false
local function later()
  if pending or not any() then
    return
  end
  pending = true
  vim.defer_fn(function()
    pending = false
    report()
  end, 120)
end
vim.api.nvim_create_autocmd({ 'TextChanged', 'TextChangedI', 'CursorMoved', 'CursorMovedI', 'BufWritePost' }, {
  group = group,
  callback = later,
})
vim.api.nvim_create_autocmd({ 'WinNew', 'WinClosed', 'WinEnter', 'TabEnter', 'BufWinEnter' }, {
  group = group,
  callback = function()
    vim.schedule(report)
  end,
})
report()
