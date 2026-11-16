-- Minimal config for trying eshvi: `eshvi -- -u dev/init.lua file.rs`
-- Puts this repo's lua/ on the runtimepath and enables rust-analyzer.
local root = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h:h')
vim.opt.runtimepath:prepend(root)
vim.opt.number = true
vim.opt.signcolumn = 'yes'
vim.opt.updatetime = 300
vim.opt.completeopt = { 'menu', 'menuone', 'noselect' }

vim.lsp.config('rust_analyzer', {
  cmd = { 'rust-analyzer' },
  filetypes = { 'rust' },
  root_markers = { 'Cargo.toml', '.git' },
})
vim.lsp.enable('rust_analyzer')

vim.api.nvim_create_autocmd('LspAttach', {
  callback = function(ev)
    vim.bo[ev.buf].omnifunc = 'v:lua.vim.lsp.omnifunc'
  end,
})

-- eshvi popups (no-ops in a terminal Neovim).
local ok, gui = pcall(require, 'gui_bridge')
if ok then
  gui.setup()
  vim.keymap.set('n', 'K', gui.hover, { desc = 'GUI hover' })
end
