let timer: ReturnType<typeof setTimeout> | undefined;
clearTimeout(timer);
timer = setTimeout(() => fn(...args), wait);
