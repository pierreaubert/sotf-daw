-- Inventory only the isolated REAPER instance used by the plugin audit.
local resource = reaper.GetResourcePath()
-- REAPER can fork via Launch Services, which need not forward the launch environment.
-- The marker confines both file writes and Quit to an explicitly prepared test resource.
local marker = io.open(resource .. "/sotf-plugin-audit-resource.txt", "r")
if not marker then return end
local marker_text = marker:read("*a")
marker:close()
if marker_text ~= "sotf-plugin-audit-20261018\n" then return end
local output = resource .. "/upmixer-probe.txt"
local expected_resource = resource
local lines = {"version=" .. reaper.GetAppVersion(), "resource=" .. resource}
local function emit()
  local file = assert(io.open(output, "w"))
  file:write(table.concat(lines, "\n") .. "\n")
  file:close()
end
if reaper.GetResourcePath() ~= expected_resource then
  lines[#lines + 1] = "error=refusing to touch a non-test REAPER resource directory"
  emit()
  return
end
lines[#lines + 1] = "script_started=true"
emit()
local ok, err = pcall(function()
  for i = 0, 100000 do
    local available, name, ident = reaper.EnumInstalledFX(i)
    if not available then break end
    if name:lower():find("upmixer", 1, true) then
      lines[#lines + 1] = "plugin=" .. name .. "|" .. ident
    end
  end
end)
lines[#lines + 1] = "inventory_ok=" .. tostring(ok)
if not ok then lines[#lines + 1] = "error=" .. tostring(err) end
emit()
reaper.defer(function() reaper.Main_OnCommand(40004, 0) end) -- Quit this empty test instance.
