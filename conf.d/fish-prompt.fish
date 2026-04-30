status is-interactive; or exit 0
command -q fish-prompt-daemon; or exit 0

set -g _fp_dir (command mktemp -d -t fish-prompt-$fish_pid)
set -g _fp_status_file $_fp_dir/status
set -g _fp_request_fifo $_fp_dir/req

command mkfifo $_fp_request_fifo

# Daemon opens the FIFO with O_RDWR (non-blocking) and exits when its parent
# (this fish) dies, via a getppid() watchdog. So fish doesn't need to hold a
# long-lived fd open.
command fish-prompt-daemon \
    --fish-pid $fish_pid \
    --status-file $_fp_status_file \
    --request-fifo $_fp_request_fifo &
disown

function _fp_request_status --on-variable PWD
    echo $PWD >$_fp_request_fifo
end

function _fp_repaint --on-signal SIGUSR1
    commandline -f repaint
end

function _fp_cleanup --on-event fish_exit
    command rm -rf $_fp_dir
end

# Trigger an initial request for the starting directory. Background it: if the
# daemon isn't ready yet, the open(O_WRONLY) on the FIFO would block.
echo $PWD >$_fp_request_fifo &
disown 2>/dev/null
