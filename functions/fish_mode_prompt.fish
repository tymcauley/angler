function fish_mode_prompt
    # fish renders this to the left of fish_prompt's first line. We use it
    # for the vi-mode indicator: a colored '[I]'/'[N]'/'[V]'/'[R]' block
    # (or a Hydro-style reverse-video block, with the default colors).
    # Auto-detects vi keybindings, so leaving _fp_show_vi_mode at the
    # default of 1 is harmless for users on emacs bindings.
    #
    # The trailing separator space is printed AFTER `set_color normal` so
    # that under reverse-video colors, the block stays tight to the symbol
    # — otherwise the separator would inherit the block's background and
    # extend the colored region by one column.

    test "$_fp_show_vi_mode" = 1; or return
    test "$fish_key_bindings" = fish_vi_key_bindings; or return

    set -l color
    set -l symbol
    switch $fish_bind_mode
        case default
            set color $_fp_color_vi_default
            set symbol $_fp_symbol_vi_default
        case insert
            set color $_fp_color_vi_insert
            set symbol $_fp_symbol_vi_insert
        case visual
            set color $_fp_color_vi_visual
            set symbol $_fp_symbol_vi_visual
        case replace replace_one
            set color $_fp_color_vi_replace
            set symbol $_fp_symbol_vi_replace
    end

    if test -n "$symbol"
        set_color $color
        printf '%s' $symbol
        set_color normal
        printf ' '
    end
end
