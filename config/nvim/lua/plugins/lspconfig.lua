return {
  'neovim/nvim-lspconfig',
  event = { "BufReadPre", "BufNewFile" },
  config = function()
    -- The fork's build (github.com/aviyn/slang-server) when it is installed: it has the fixes that aren't released yet.
    -- `slang_loads_safe` tells the signal trace that this server can be asked for loads freely.
    local patched_slang_server = vim.fn.expand("~/tools/slang-server-patched/slang-server")
    vim.g.slang_loads_safe = vim.fn.executable(patched_slang_server) == 1
    local slang_server = vim.g.slang_loads_safe and patched_slang_server or vim.fn.expand("~/tools/slang-server/slang-server")
    vim.lsp.config("slang-server", {
      -- In ~/tools on every machine (the home folder differs).
      cmd = { slang_server },
      root_markers = { ".git", ".slang" },
      filetypes = {
        "systemverilog",
        "verilog",
      },
    })
    -- svtrace answers the signal trace and the hierarchy's scopes from slang's elaborated AST (crates/svtrace);
    -- slang-server keeps everything else. Eshvi sets its client up itself from the pair that ships beside the
    -- app ($ESHVI_BIN, `eshvi.slang`). Only the development pair in ~/tools is set here, which wins over it.
    local svtrace_dev = {
      svtrace = vim.fn.expand("~/tools/svtrace/svtrace"),
      slang = vim.fn.expand("~/tools/slang/slang"),
    }
    local svtrace_in_tools = vim.fn.executable(svtrace_dev.svtrace) == 1 and vim.fn.executable(svtrace_dev.slang) == 1
    if svtrace_in_tools then
      vim.lsp.config("svtrace", {
        cmd = { svtrace_dev.svtrace, "--slang", svtrace_dev.slang },
        root_markers = { ".slang" },
        filetypes = {
          "systemverilog",
          "verilog",
        },
      })
    end
    -- rust-analyzer from rustup (`rustup component add rust-analyzer`);
    -- clippy lints on save instead of plain `cargo check`.
    vim.lsp.config("rust_analyzer", {
      settings = {
        ["rust-analyzer"] = {
          check = { command = "clippy" },
        },
      },
    })
    vim.diagnostic.config({
      signs = {
        text = {
          [vim.diagnostic.severity.ERROR] = "●",
          [vim.diagnostic.severity.WARN]  = "●",
          [vim.diagnostic.severity.INFO]  = "●",
          [vim.diagnostic.severity.HINT]  = "●",
        },
      },
    })
    vim.lsp.enable({'pylsp', 'lua_ls', 'clangd', 'slang-server', 'rust_analyzer'})
    if svtrace_in_tools then
      vim.lsp.enable('svtrace')
    end
  end
}
