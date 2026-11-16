-- A buffer's document symbols, as one tree whichever form the language server
-- answers in. Used by the Outline (`outline.lua`) and the picker's symbols
-- (`picker.lua`, which flattens it). Loaded with any config.
--
--   Eshvi.symbols.request(buf, function(tree) end)  textDocument/documentSymbol of every client
--     that supports it; the first client's non-empty answer, as a tree (no
--     argument: no client supports it; an empty tree: no symbols)
--   Eshvi.symbols.convert(results, lines)  the same from buf_request_all's results
--   Eshvi.symbols.flatten(tree)            in source order, each with its depth
--   Eshvi.symbols.at(flat, cursor)         the entry to start on for a { line, byte column }
--
-- A symbol: { name, detail, kind (SymbolKind's name), line, col (the name's
-- start: line from 1, byte column from 0), start_line, start_col, end_line,
-- end_col (its range, bytes, lines from 1), flat (it came from a flat
-- SymbolInformation list), container (that list's containerName), children }.
local METHOD = 'textDocument/documentSymbol'

_G.Eshvi = _G.Eshvi or {}
local S = {}
Eshvi.symbols = S

S.method = METHOD

function S.kind_name(kind)
  return vim.lsp.protocol.SymbolKind[kind] or 'Unknown'
end

-- Where the character `character` of `line` (UTF-8 bytes from 0), as the
-- server counts it.
local function byte_of(line, character, encoding)
  if not line then
    return character
  end
  local ok, idx = pcall(vim.str_byteindex, line, encoding or 'utf-16', character, false)
  return (ok and idx) or #line
end

local function client_encoding(client_id)
  local client = vim.lsp.get_client_by_id(client_id)
  return client and client.offset_encoding or 'utf-16'
end

local function sorted_ids(results)
  local ids = vim.tbl_keys(results)
  table.sort(ids)
  return ids
end

-- A DocumentSymbol[] (nested) into nodes, positions as the buffer's bytes.
local function convert_nested(list, lines, encoding)
  local out = {}
  for _, s in ipairs(list) do
    local range = s.range
    local sel = s.selectionRange or s.range
    if range and sel then
      local node = {
        name = s.name,
        detail = s.detail,
        kind = S.kind_name(s.kind),
        line = sel.start.line + 1,
        col = byte_of(lines[sel.start.line + 1], sel.start.character, encoding),
        start_line = range.start.line + 1,
        start_col = byte_of(lines[range.start.line + 1], range.start.character, encoding),
        end_line = range['end'].line + 1,
        end_col = byte_of(lines[range['end'].line + 1], range['end'].character, encoding),
        children = s.children and convert_nested(s.children, lines, encoding) or {},
      }
      table.insert(out, node)
    end
  end
  return out
end

local function holds(outer, inner)
  local starts = outer.start_line < inner.start_line or (outer.start_line == inner.start_line and outer.start_col <= inner.start_col)
  local ends = outer.end_line > inner.end_line or (outer.end_line == inner.end_line and outer.end_col >= inner.end_col)
  return starts and ends
end

-- A SymbolInformation[] (flat) into a tree by range containment: a symbol
-- inside another's range is its child. (`containerName` is only a name.)
local function convert_flat(list, lines, encoding)
  local all = {}
  for i, s in ipairs(list) do
    local range = s.location and s.location.range
    if range then
      table.insert(all, {
        name = s.name,
        kind = S.kind_name(s.kind),
        line = range.start.line + 1,
        col = byte_of(lines[range.start.line + 1], range.start.character, encoding),
        start_line = range.start.line + 1,
        start_col = byte_of(lines[range.start.line + 1], range.start.character, encoding),
        end_line = range['end'].line + 1,
        end_col = byte_of(lines[range['end'].line + 1], range['end'].character, encoding),
        flat = true,
        container = s.containerName ~= '' and s.containerName or nil,
        children = {},
        order = i,
      })
    end
  end
  -- Where they start, the bigger first (the container before what it holds).
  table.sort(all, function(a, b)
    if a.start_line ~= b.start_line then
      return a.start_line < b.start_line
    end
    if a.start_col ~= b.start_col then
      return a.start_col < b.start_col
    end
    if a.end_line ~= b.end_line then
      return a.end_line > b.end_line
    end
    if a.end_col ~= b.end_col then
      return a.end_col > b.end_col
    end
    return a.order < b.order
  end)
  local roots, stack = {}, {}
  for _, s in ipairs(all) do
    while #stack > 0 and not holds(stack[#stack], s) do
      table.remove(stack)
    end
    table.insert(#stack > 0 and stack[#stack].children or roots, s)
    table.insert(stack, s)
  end
  return roots
end

-- The tree of the first client (by id) with symbols, from the results of
-- `buf_request_all`; empty when none has any.
function S.convert(results, lines)
  for _, id in ipairs(sorted_ids(results)) do
    local res = results[id]
    local list = res.result
    if list and #list > 0 then
      local encoding = client_encoding(id)
      if list[1].location ~= nil then
        return convert_flat(list, lines, encoding)
      end
      return convert_nested(list, lines, encoding)
    end
  end
  return {}
end

function S.request(buf, callback)
  if #vim.lsp.get_clients({ bufnr = buf, method = METHOD }) == 0 then
    return callback()
  end
  vim.lsp.buf_request_all(buf, METHOD, function()
    return { textDocument = vim.lsp.util.make_text_document_params(buf) }
  end, function(results)
    if not vim.api.nvim_buf_is_valid(buf) then
      return
    end
    callback(S.convert(results, vim.api.nvim_buf_get_lines(buf, 0, -1, false)))
  end)
end

-- The tree in source order, each node with its `depth` and `order`.
function S.flatten(tree)
  local out = {}
  local function walk(list, depth)
    for _, s in ipairs(list) do
      s.depth = depth
      s.order = #out
      table.insert(out, s)
      walk(s.children, depth + 1)
    end
  end
  walk(tree, 0)
  return out
end

-- Kinds that hold other things: the cursor in one of them is "in" it, where a
-- parameter or a local is a smaller thing inside.
local CONTAINERS = {
  Function = true, Method = true, Constructor = true, Class = true, Module = true, Namespace = true,
  Package = true, Struct = true, Interface = true, Enum = true, File = true,
}

-- The entry to start on: the innermost one whose range holds the cursor,
-- among the containers (a function, a class…) if any holds it, else the
-- innermost of any kind; if none holds it, the last that starts before it.
-- `flat` is in source order (`flatten`), `cursor` a { line from 1, byte
-- column from 0 }. Returns its index.
function S.at(flat, cursor)
  local function starts_before(s)
    return s.start_line < cursor[1] or (s.start_line == cursor[1] and s.start_col <= cursor[2])
  end
  local function ends_after(s)
    return s.end_line > cursor[1] or (s.end_line == cursor[1] and s.end_col >= cursor[2])
  end
  local any, container, before
  for i, s in ipairs(flat) do
    if starts_before(s) then
      before = i
      if ends_after(s) then
        -- (Holding the cursor, the ones in source order nest: the last is the innermost.)
        any = i
        if CONTAINERS[s.kind] then
          container = i
        end
      end
    end
  end
  return container or any or before
end
