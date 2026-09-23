require("ffi_wrapper")
require("ffi/loadlib")
local HB = require("ffi/harfbuzz")
local FT = require("ffi/freetype")

describe("HarfBuzz vertical features", function()
    it("reports no vert/vrt2 for a font without vertical GSUB (DroidSansMono)", function()
        local ftsize = FT.newFaceSize("./fonts/droid/DroidSansMono.ttf", 16*64, 0)
        local hbface = HB.hb_ft_face_create_referenced(ftsize.face)
        local has_vert, has_vrt2 = hbface:hasVerticalFeatures()
        assert.is_false(has_vert)
        assert.is_false(has_vrt2)
        hbface:destroy()
        ftsize:done()
    end)

    it("reports vert/vrt2 for a font that contains them (stubbed fallback)", function()
        -- Simulate the Kobo condition: table-wide enumeration returns 0,
        -- per-script union contains vert/vrt2. This is the regression that
        -- hit Noto Sans CJK JP on Kobo nightly (GSUB count 0 table-wide, 7
        -- scripts, per-script union true,true).
        local ffi = require("ffi")
        local ftsize = FT.newFaceSize("./fonts/droid/DroidSansMono.ttf", 16*64, 0)
        local hbface = HB.hb_ft_face_create_referenced(ftsize.face)

        -- Save originals (internal callers route through the HB proxy,
        -- so these stubs apply; raw-namespace slots are immutable)
        local orig_table_feat = HB.hb_ot_layout_table_get_feature_tags
        local orig_table_script = HB.hb_ot_layout_table_get_script_tags
        local orig_script_lang = HB.hb_ot_layout_script_get_language_tags
        local orig_lang_find = HB.hb_ot_layout_language_find_feature

        -- Force table-wide to return 0 (broken build)
        HB.hb_ot_layout_table_get_feature_tags = function(face, tag, off, count, tags)
            count[0] = 0
            return 0
        end
        -- Report 1 fake script
        HB.hb_ot_layout_table_get_script_tags = function(face, tag, off, count, tags)
            if tags == nil then count[0] = 1; return 1 end
            tags[0] = 0x44464C54 -- 'DFLT'
            count[0] = 1
            return 1
        end
        HB.hb_ot_layout_script_get_language_tags = function(face, tag, si, off, count, tags)
            if tags == nil then count[0] = 0; return 0 end
            count[0] = 0
            return 0
        end
        -- Current implementation uses direct find_feature (not enumeration)
        HB.hb_ot_layout_language_find_feature = function(face, tag, si, lang, ftag, idx)
            if ftag == 0x76657274 or ftag == 0x76727432 then return 1 end -- 'vert'/'vrt2'
            return 0
        end

        local has_vert, has_vrt2 = hbface:hasVerticalFeatures()
        assert.is_true(has_vert)
        assert.is_true(has_vrt2)

        -- Restore
        HB.hb_ot_layout_table_get_feature_tags = orig_table_feat
        HB.hb_ot_layout_table_get_script_tags = orig_table_script
        HB.hb_ot_layout_script_get_language_tags = orig_script_lang
        HB.hb_ot_layout_language_find_feature = orig_lang_find

        hbface:destroy()
        ftsize:done()
    end)

    it("real Noto Sans CJK JP reports vert/vrt2 when fixture is present", function()
        local path = "/tmp/notocjk/NotoSansCJKjp-Regular.otf"
        local f = io.open(path, "r")
        if not f then pending("Noto fixture not present"); return end
        f:close()
        local ftsize = FT.newFaceSize(path, 16*64, 0)
        local hbface = HB.hb_ft_face_create_referenced(ftsize.face)
        local has_vert, has_vrt2 = hbface:hasVerticalFeatures()
        assert.is_true(has_vert, "Noto should have vert")
        assert.is_true(has_vrt2, "Noto should have vrt2")
        hbface:destroy()
        ftsize:done()
    end)
end)
