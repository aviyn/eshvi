-- Slang (SystemVerilog) support as part of the app, for the own config
-- (`[neovim] config = "own"`): the Hierarchy and Trace views, `:EshviSlang`, the
-- search and svtrace's language server (`slang/`). Modules `eshvi.slang`,
-- `eshvi.slang.hierarchy` and `eshvi.slang.trace`, named for their files so
-- tracebacks read as before and the health check's float log counts them as
-- Eshvi's. A config that loaded an `eshvi.slang` of its own keeps it. Opt out:
-- `vim.g.eshvi_slang = false`. The views come only when a slang-server or svtrace
-- client attaches.
local sources = ...

if vim.g.eshvi_slang == false or package.loaded['eshvi.slang'] then
  return
end

for name, src in pairs(sources) do
  local module = name == 'init' and 'eshvi.slang' or ('eshvi.slang.' .. name)
  package.preload[module] = function()
    return assert(loadstring(src, '@eshvi/slang/' .. name .. '.lua'))()
  end
end

require('eshvi.slang').setup()
