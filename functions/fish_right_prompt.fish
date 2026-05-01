function fish_right_prompt
    # Intentionally empty. fish_prompt embeds right-aligned content (time,
    # command duration) on its first line directly, so the right-prompt
    # appears on the same row as the path/git info even in our multi-line
    # layout — which fish's built-in fish_right_prompt would not.
end
