-- Forwards LSP hover, signature help and diagnostics to the eshvi GUI,
-- which renders them as native popups.
--
-- Inactive unless the GUI has set g:gui_bridge_chan, so the same config is
-- harmless in a terminal Neovim (hover() then falls back to the built-in).
--
--   require('gui_bridge').setup()
--   vim.keymap.set('n', 'K', require('gui_bridge').hover)
--
-- Global vim.lsp.handlers are never touched.

local M = {}

local config = {
  -- Debounce for signature help updates while typing/moving in insert mode.
  signature_debounce_ms = 80,
  signature = true,
  diagnostics = true,
}

local function chan()
  local c = vim.g.gui_bridge_chan
  if type(c) == 'number' and c > 0 then
    return c
  end
  return nil
end

local function notify(method, payload)
  local c = chan()
  if c then
    vim.rpcnotify(c, method, payload)
  end
end

--- Close GUI popups. `kind` is 'hover', 'signature', 'diagnostic' or nil (all).
function M.close(kind)
  notify('gui_popup_close', { kind = kind })
end

local function markup_to_markdown(contents)
  if contents == nil then
    return ''
  end
  local lines = vim.lsp.util.convert_input_to_markdown_lines(contents)
  return vim.trim(table.concat(lines, '\n'))
end

local function position_params(client, bufnr)
  return vim.lsp.util.make_position_params(vim.fn.bufwinid(bufnr), client.offset_encoding)
end

local function has_method(bufnr, method)
  return #vim.lsp.get_clients({ bufnr = bufnr, method = method }) > 0
end

--- Show hover documentation from every attached client in a GUI popup.
function M.hover()
  if not chan() then
    return vim.lsp.buf.hover()
  end
  local bufnr = vim.api.nvim_get_current_buf()
  if not has_method(bufnr, 'textDocument/hover') then
    return
  end
  vim.lsp.buf_request_all(bufnr, 'textDocument/hover', position_params, function(results)
    local parts = {}
    for _, res in pairs(results) do
      if res.result and res.result.contents then
        local md = markup_to_markdown(res.result.contents)
        if md ~= '' then
          table.insert(parts, md)
        end
      end
    end
    if #parts == 0 then
      vim.notify('No information available', vim.log.levels.INFO)
      return
    end
    notify('gui_hover', { contents = table.concat(parts, '\n\n---\n\n'), kind = 'markdown' })
  end)
end

-- Signature help ------------------------------------------------------------

local sig_timer = nil
local sig_active = false

-- Convert one LSP SignatureInformation into the GUI payload. Parameter
-- ranges are 0-based byte offsets into `label` (end exclusive).
local function convert_signature(sig, encoding)
  local label = sig.label or ''
  local out = {
    label = label,
    documentation = markup_to_markdown(sig.documentation),
    parameters = {},
    active_parameter = sig.activeParameter,
  }
  local search_from = 1
  for _, p in ipairs(sig.parameters or {}) do
    local s, e
    if type(p.label) == 'table' then
      s = vim.str_byteindex(label, encoding, p.label[1], false)
      e = vim.str_byteindex(label, encoding, p.label[2], false)
    elseif type(p.label) == 'string' then
      local fs, fe = label:find(p.label, search_from, true)
      if fs then
        s, e = fs - 1, fe
        search_from = fe + 1
      end
    end
    table.insert(out.parameters, {
      start = s or 0,
      ['end'] = e or 0,
      documentation = markup_to_markdown(p.documentation),
    })
  end
  return out
end

local function request_signature()
  if not chan() then
    return
  end
  local bufnr = vim.api.nvim_get_current_buf()
  if not has_method(bufnr, 'textDocument/signatureHelp') then
    return
  end
  vim.lsp.buf_request_all(bufnr, 'textDocument/signatureHelp', position_params, function(results)
    for client_id, res in pairs(results) do
      local help = res.result
      if help and help.signatures and #help.signatures > 0 then
        local client = vim.lsp.get_client_by_id(client_id)
        local encoding = client and client.offset_encoding or 'utf-16'
        local sigs = {}
        for _, s in ipairs(help.signatures) do
          table.insert(sigs, convert_signature(s, encoding))
        end
        sig_active = true
        notify('gui_signature', {
          signatures = sigs,
          active_signature = help.activeSignature or 0,
          active_parameter = help.activeParameter,
        })
        return
      end
    end
    if sig_active then
      sig_active = false
      M.close('signature')
    end
  end)
end

local function schedule_signature()
  if sig_timer == nil then
    sig_timer = vim.uv.new_timer()
  end
  sig_timer:stop()
  sig_timer:start(config.signature_debounce_ms, 0, vim.schedule_wrap(request_signature))
end

--- Request signature help now (e.g. map it to <C-s> in insert mode).
M.signature_help = request_signature

local function trigger_chars(bufnr)
  local chars = {}
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr, method = 'textDocument/signatureHelp' })) do
    local provider = client.server_capabilities.signatureHelpProvider or {}
    for _, c in ipairs(provider.triggerCharacters or {}) do
      chars[c] = true
    end
    for _, c in ipairs(provider.retriggerCharacters or {}) do
      chars[c] = true
    end
  end
  return chars
end

-- Diagnostics -----------------------------------------------------------------

local severity_names = { 'Error', 'Warning', 'Info', 'Hint' }

-- Where the cursor is: buffer, line and column.
local function here()
  local pos = vim.api.nvim_win_get_cursor(0)
  return vim.api.nvim_get_current_buf() .. ':' .. pos[1] .. ':' .. pos[2]
end

-- Where the user dismissed the diagnostics popup (Esc): not shown again
-- there (a later CursorHold, the server checking again) until the cursor
-- moves on.
local dismissed_at = nil

--- The user closed the diagnostics popup (the GUI calls this on Esc).
function M.dismiss_diagnostics()
  dismissed_at = here()
end

--- Send the diagnostics under the cursor to the GUI (in Normal mode, and
--- not where the user dismissed them).
function M.show_diagnostics()
  if not chan() or vim.api.nvim_get_mode().mode ~= 'n' or dismissed_at == here() then
    return
  end
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line_diags = vim.diagnostic.get(0, { lnum = row - 1 })
  local under = {}
  for _, d in ipairs(line_diags) do
    local end_col = d.end_col or d.col
    local end_lnum = d.end_lnum or d.lnum
    local covers = (d.lnum < row - 1 or col >= d.col) and (end_lnum > row - 1 or col < math.max(end_col, d.col + 1))
    if covers then
      table.insert(under, d)
    end
  end
  if #under == 0 then
    under = line_diags
  end
  if #under == 0 then
    return
  end
  local out = {}
  for _, d in ipairs(under) do
    table.insert(out, {
      severity = severity_names[d.severity] or 'Error',
      message = d.message,
      source = d.source or '',
      code = d.code and tostring(d.code) or '',
    })
  end
  notify('gui_diagnostic', { diagnostics = out })
end

-- Setup -----------------------------------------------------------------------

function M.setup(opts)
  config = vim.tbl_extend('force', config, opts or {})
  local group = vim.api.nvim_create_augroup('gui_bridge', { clear = true })

  vim.api.nvim_create_autocmd({ 'CursorMoved', 'InsertLeave', 'BufLeave', 'WinScrolled' }, {
    group = group,
    callback = function(ev)
      if not chan() then
        return
      end
      if ev.event == 'InsertLeave' or ev.event == 'BufLeave' then
        sig_active = false
      end
      M.close()
    end,
  })

  if config.signature then
    vim.api.nvim_create_autocmd('InsertCharPre', {
      group = group,
      callback = function(ev)
        if chan() and trigger_chars(ev.buf)[vim.v.char] then
          schedule_signature()
        end
      end,
    })
    vim.api.nvim_create_autocmd({ 'CursorMovedI', 'TextChangedI' }, {
      group = group,
      callback = function()
        -- Only keep an open signature popup in sync; opening is up to the
        -- trigger characters.
        if chan() and sig_active then
          schedule_signature()
        end
      end,
    })
  end

  if config.diagnostics then
    -- Where the cursor last rested. Diagnostics often arrive after
    -- CursorHold (the server is still checking), so a change while the
    -- cursor is still resting there shows them too.
    local held_at = nil
    vim.api.nvim_create_autocmd('CursorHold', {
      group = group,
      callback = function()
        held_at = here()
        M.show_diagnostics()
      end,
    })
    vim.api.nvim_create_autocmd({ 'CursorMoved', 'InsertEnter' }, {
      group = group,
      callback = function(ev)
        held_at = nil
        if ev.event == 'CursorMoved' and dismissed_at ~= here() then
          dismissed_at = nil
        end
        -- Typing: out of the way (of the completion menu too).
        if ev.event == 'InsertEnter' and chan() then
          M.close('diagnostic')
        end
      end,
    })
    vim.api.nvim_create_autocmd('DiagnosticChanged', {
      group = group,
      callback = function()
        if held_at and held_at == here() and vim.api.nvim_get_mode().mode == 'n' then
          M.show_diagnostics()
        end
      end,
    })
  end
end

return M
