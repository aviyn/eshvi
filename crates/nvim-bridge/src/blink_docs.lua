-- blink.cmp's documentation in the GUI's hover popup (`gui_docs`): rendered
-- Markdown, links that open, placed beside the menu by the GUI. blink's own
-- window stays closed; its keys (show, hide, scroll) work on the popup. It
-- has no hook for this, so its documentation module's functions are replaced;
-- if they aren't what's expected, blink's window stays.
-- `vim.g.eshvi_blink_docs = false` keeps blink's window.
local chan = ...

local function patch()
  local ok, docs = pcall(require, 'blink.cmp.completion.windows.documentation')
  local ok_menu, menu = pcall(require, 'blink.cmp.completion.windows.menu')
  local ok_sources, sources = pcall(require, 'blink.cmp.sources.lib')
  if not (ok and ok_menu and ok_sources and type(docs.show_item) == 'function' and docs.win and menu.win) then
    return
  end
  local shown = false
  local function close()
    if docs.auto_show_timer then
      docs.auto_show_timer:stop()
    end
    docs.shown_item = nil
    if shown then
      shown = false
      vim.rpcnotify(chan, 'gui_docs', { contents = '', win = 0 })
    end
  end
  docs.close = close
  docs.win.is_open = function()
    return shown
  end
  docs.win.close = close
  docs.update_position = function() end
  docs.scroll_up = function(amount)
    vim.rpcnotify(chan, 'gui_docs_scroll', { rows = -(amount or 4) })
  end
  docs.scroll_down = function(amount)
    vim.rpcnotify(chan, 'gui_docs_scroll', { rows = amount or 4 })
  end
  docs.show_item = function(context, item)
    if docs.auto_show_timer then
      docs.auto_show_timer:stop()
    end
    if item == nil or not menu.win:is_open() then
      return close()
    end
    sources.resolve(context, item):map(function(resolved)
      local contents = Eshvi.item_markdown(resolved)
      local win = menu.win:get_win()
      if contents == '' or not win then
        return close()
      end
      docs.shown_item = resolved
      shown = true
      vim.rpcnotify(chan, 'gui_docs', { contents = contents, win = win })
    end):catch(function(err)
      vim.notify(tostring(err), vim.log.levels.ERROR, { title = 'blink.cmp' })
    end)
  end
  return true
end

-- blink may load late (on InsertEnter), so try until it's there.
local group = vim.api.nvim_create_augroup('eshvi_blink_docs', { clear = true })
vim.api.nvim_create_autocmd({ 'InsertEnter', 'CmdlineEnter', 'TextChangedI', 'CursorMovedI' }, {
  group = group,
  callback = function()
    if vim.g.eshvi_blink_docs == false then
      return true
    end
    if package.loaded['blink.cmp'] then
      pcall(patch)
      return true -- once
    end
  end,
})
