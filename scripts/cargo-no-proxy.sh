#!/bin/bash
# Unset all proxy variables
unset HTTP_PROXY
unset HTTPS_PROXY
unset http_proxy
unset https_proxy
unset ftp_proxy
unset FTP_PROXY
unset no_proxy
unset NO_PROXY

# Set empty values
export HTTP_PROXY=""
export HTTPS_PROXY=""
export http_proxy=""
export https_proxy=""
export ftp_proxy=""
export FTP_PROXY=""
export no_proxy=""
export NO_PROXY=""

# Execute cargo with all arguments
exec cargo "$@"
