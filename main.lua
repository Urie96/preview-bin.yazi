--- @since 26.8.15

-- NOTE: this plugin must not be named `preview`: `preview` is a built-in Yazi
-- component name that the loader serves from its in-memory cache, which shadows
-- `plugins/preview.yazi` and makes `peek` fail with "error converting Lua nil to
-- table". The directory is `preview-bin.yazi` and `yazi.toml` uses
-- `run = "preview-bin"`.

-- The `preview` executable is looked up from `PATH`; all of its arguments are
-- fixed here, so users don't pass anything in `yazi.toml`.

local CMD = "preview"

local M = {}

local function fail(job, msg) ya.preview_widget(job, ui.Text.parse(msg):area(job.area):wrap(ui.Wrap.YES)) end

local function strip(line) return (line:gsub("[\r\n]+$", "")) end

local loading_text = (function()
	local i = 1
	local frames = { "▱▱▱", "▰▱▱", "▰▰▱", "▰▰▰", "▰▰▱", "▰▱▱", "▱▱▱" }
	return function()
		i = (i % #frames) + 1
		return "Loading" .. frames[i]
	end
end)()

local function loading(job)
	local area = job.area
	local w = math.min(10, area.w)
	local x = area.x + math.max(0, math.floor((area.w - w) / 2))
	ya.preview_widget(
		job,
		ui.Text.parse(loading_text()):area(ui.Rect {
			x = x,
			y = area.y + math.floor(area.h / 2),
			w = w,
			h = 1,
		})
	)
end

function M:peek(job)
	-- One skip unit is half a page: this keeps text scrolling comfortable while
	-- still moving exactly one page for PDFs and one thumbnail step for videos.
	local half = math.max(1, math.floor(job.area.h / 2))
	local offset = job.skip * half
	local limit = math.max(1, job.area.h)

	local child, err = Command(CMD)
		:arg({
			"--mime",
			job.mime,
			"--width",
			tostring(job.area.w),
			"--height",
			tostring(job.area.h),
			"--offset",
			tostring(job.skip),
			tostring(job.file.path),
		})
		:stdout(Command.PIPED)
		:stderr(Command.PIPED)
		:spawn()

	if not child then
		return fail(job, "Failed to start `preview`: " .. tostring(err))
	end

	local i, outs, errs = 0, {}, {}
	local killed = false

	while true do
		local line, event = child:read_line_with { timeout = 300 }
		if event == 3 then
			-- Nothing has been produced yet, which usually means the preview is
			-- being generated in the cache.
			if i == 0 and #errs == 0 then
				loading(job)
			end
		elseif event == 2 then
			break
		elseif event == 1 then
			errs[#errs + 1] = line
		else
			if i == 0 then
				local path = strip(line):match("^@@image:(.+)$")
				if path then
					child:start_kill()
					local _, image_err = ya.image_show(Url(path), job.area)
					return ya.preview_widget(job, image_err)
				end
			end

			i = i + 1
			if i > offset then
				outs[#outs + 1] = line
			end
			if #outs >= limit then
				child:start_kill()
				killed = true
				break
			end
		end
	end

	if not killed then
		local status, wait_err = child:wait()
		if not status then
			return fail(job, "Failed to wait for `preview`: " .. tostring(wait_err))
		end

		if status.code == 3 then
			if job.skip == 0 then
				return fail(job, "Preview is out of bounds")
			end
			return ya.emit("peek", {
				job.skip - 1,
				only_if = job.file.url,
				upper_bound = true,
			})
		elseif not status.success and i == 0 and #errs == 0 then
			return fail(job, string.format("`preview` exited with code %s", tostring(status.code)))
		end
	end

	-- The file ended before the requested skip; snap back to a valid position.
	local remaining = i - offset
	if job.skip > 0 and i > 0 and remaining < limit then
		local next_skip = math.ceil(math.max(0, i - limit) / half)
		if next_skip >= job.skip then
			next_skip = job.skip - 1
		end
		return ya.emit("peek", {
			next_skip,
			only_if = job.file.url,
			upper_bound = true,
		})
	end

	local text = table.concat(errs, "") .. table.concat(outs, "")
	text = text:gsub("\t", string.rep(" ", rt.preview.tab_size))
	ya.preview_widget(job, ui.Text.parse(text):area(job.area))
end

function M:seek(job)
	local h = cx.active.current.hovered
	if not h or h.url ~= job.file.url then
		return
	end

	local step = 0
	if job.units > 0 then
		step = 1
	elseif job.units < 0 then
		step = -1
	end
	if step ~= 0 then
		ya.emit("peek", {
			math.max(0, cx.active.preview.skip + step),
			only_if = job.file.url,
		})
	end
end

return M
