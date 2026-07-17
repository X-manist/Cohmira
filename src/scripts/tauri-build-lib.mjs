export function npmInvocation(npmArgs, {
  platform = process.platform,
  comSpec = process.env.ComSpec,
} = {}) {
  if (platform === 'win32') {
    return {
      command: comSpec || 'cmd.exe',
      args: ['/d', '/s', '/c', 'npm.cmd', ...npmArgs],
    };
  }

  return {
    command: 'npm',
    args: npmArgs,
  };
}

export function frontendBuildInvocation(options = {}) {
  return npmInvocation(['run', 'build'], options);
}
