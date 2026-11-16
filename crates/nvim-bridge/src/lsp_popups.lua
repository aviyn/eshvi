-- LSP hover, signature help and diagnostics as Eshvi's native popups, for the
-- own config (`[neovim] config = "own"`). The plugin is the repo's
-- `lua/gui_bridge`, shipped in the app so a config needs no checkout of this
-- repo: `require('gui_bridge')` works as before, and K is mapped to its hover
-- unless the config maps K itself (globally). A config that already loaded a
-- `gui_bridge` of its own keeps it. Opt out: `vim.g.eshvi_lsp_popups = false`.
local src = ...

if vim.g.eshvi_lsp_popups == false or package.loaded.gui_bridge then
  return
end

package.preload.gui_bridge = function()
  -- Named as the plugin's file, so tracebacks read as before and the health
  -- check's float log counts it as Eshvi's.
  return assert(loadstring(src, '@eshvi/lua/gui_bridge/init.lua'))()
end

local gui = require('gui_bridge')
gui.setup()

local function mine(m)
  return m.lhs == 'K'
end
if #vim.tbl_filter(mine, vim.api.nvim_get_keymap('n')) == 0 then
  vim.keymap.set('n', 'K', gui.hover, { desc = 'LSP hover' })
  -- A buffer whose server attached before this ran has Neovim's default K,
  -- which would win over the global one. Its later buffers don't get it: it is
  -- only set where K isn't mapped.
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    for _, m in ipairs(vim.api.nvim_buf_get_keymap(buf, 'n')) do
      if mine(m) and m.desc == 'vim.lsp.buf.hover()' then
        pcall(vim.keymap.del, 'n', 'K', { buffer = buf })
      end
    end
  end
end
