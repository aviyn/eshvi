-- Makes `:checkhealth eshvi` findable: `health.lua` was written to
-- `<runtime>/lua/eshvi/health.lua`; the runtime folder joins the runtimepath on
-- VimEnter (lazy.nvim resets the runtimepath during its setup, before that), or
-- at once if Neovim has started. Also `:EshviHealth`. Called with that folder.
local runtime = ...

local function add()
  if not vim.tbl_contains(vim.opt.rtp:get(), runtime) then
    vim.opt.rtp:append(runtime)
  end
end
if vim.v.vim_did_enter == 1 then
  add()
else
  vim.api.nvim_create_autocmd('VimEnter', {
    group = vim.api.nvim_create_augroup('EshviHealth', { clear = true }),
    once = true,
    callback = add,
  })
end

vim.api.nvim_create_user_command('EshviHealth', function()
  add()
  vim.cmd('checkhealth eshvi')
end, {})
