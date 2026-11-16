-- Passed to nvim with --cmd, so it runs before the user's config. Keeps the
-- built-in style vim.notify (messages, shown by the GUI as toasts) even when
-- a plugin assigns its own, e.g. nvim-notify. Only used inside eshvi.
-- Full-line comments are stripped and lines joined, so no inline comments.
local function notify(msg, level, o)
  msg = tostring(msg)
  if type(o) == 'table' and o.title then
    msg = o.title .. ': ' .. msg
  end
  if level == vim.log.levels.ERROR then
    vim.api.nvim_echo({ { msg } }, true, { err = true })
  elseif level == vim.log.levels.WARN then
    vim.api.nvim_echo({ { msg, 'WarningMsg' } }, true, {})
  else
    vim.api.nvim_echo({ { msg } }, true, {})
  end
end
local mt = getmetatable(vim) or {}
local index, newindex = mt.__index, mt.__newindex
rawset(vim, 'notify', nil)
mt.__index = function(t, k)
  if k == 'notify' then
    return notify
  end
  if type(index) == 'function' then
    return index(t, k)
  elseif index then
    return index[k]
  end
end
mt.__newindex = function(t, k, v)
  if k == 'notify' then
    return
  end
  if newindex then
    return newindex(t, k, v)
  end
  rawset(t, k, v)
end
setmetatable(vim, mt)
