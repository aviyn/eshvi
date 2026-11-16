-- Keep buffers in sync with file operations done in the GUI explorer.
--   'rename', from, to : buffers of `from` (or files under it) follow the move
--   'delete', path     : unmodified buffers of `path` (or under it) are wiped
--   'cd', path         : change the global working directory
--   'root', path       : the explorer's root (g:eshvi_root), where new
--                        panel terminals start
local op, a, b = ...
-- Compare real paths with forward slashes: symlinks resolved (/var ->
-- /private/var on macOS), and on Windows long names instead of 8.3 ones
-- (RUNNER~1) and `/` instead of `\`. The file itself may be gone, so
-- resolve its folder.
local function real(p)
  if not p then
    return p
  end
  p = vim.fs.normalize(p)
  local dir = vim.uv.fs_realpath(vim.fs.dirname(p))
  return vim.fs.normalize(dir and vim.fs.joinpath(dir, vim.fs.basename(p)) or p)
end
a, b = real(a), real(b)
if op == 'cd' then
  vim.cmd.cd(vim.fn.fnameescape(a))
  return
end
if op == 'root' then
  vim.g.eshvi_root = a
  return
end
local function under(p, root)
  return p == root or vim.startswith(p, root .. '/')
end
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
  local name = vim.api.nvim_buf_get_name(buf)
  if name ~= '' and vim.bo[buf].buftype == '' then
    local p = real(vim.fn.fnamemodify(name, ':p'))
    if op == 'rename' and under(p, a) then
      -- (A name another buffer has is refused: this one keeps its old
      -- name, and the buffers after it still follow.)
      local named = pcall(vim.api.nvim_buf_set_name, buf, b .. p:sub(#a + 1))
      if named and not vim.bo[buf].modified then
        -- Re-read under the new name (also clears "not edited").
        vim.api.nvim_buf_call(buf, function()
          vim.cmd('silent! edit!')
        end)
      end
    elseif op == 'delete' and under(p, a) and not vim.bo[buf].modified then
      pcall(vim.api.nvim_buf_delete, buf, { force = false })
    end
  end
end
