function fish_mode_prompt
    # fish renders this to the left of fish_prompt's first line. We use it
    # for the vi-mode indicator: a colored '[I]'/'[N]'/'[V]'/'[R]' block.
    # Auto-detects vi keybindings, so leaving _fp_show_vi_mode at the default
    # of 1 is harmless for users on emacs bindings.

    test "$_fp_show_vi_mode" = 1; or return
    test "$fish_key_bindings" = fish_vi_key_bindings; or return

    switch $fish_bind_mode
        case default
            set_color $_fp_color_vi_default
            printf '%s ' $_fp_symbol_vi_default
        case insert
            set_color $_fp_color_vi_insert
            printf '%s ' $_fp_symbol_vi_insert
        case visual
            set_color $_fp_color_vi_visual
            printf '%s ' $_fp_symbol_vi_visual
        case replace replace_one
            set_color $_fp_color_vi_replace
            printf '%s ' $_fp_symbol_vi_replace
    end
    set_color normal
end
